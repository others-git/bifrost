// ─────────────────────────────────────────────────────────────────────────────
// Board backgrounds — the atmospheric layer behind a board's widgets.
//
// A background is either an **upload** (image / gif / short video loop served
// from `/api/dashboards/{id}/background/media`) or a **preset**: a living scene
// drawn on one canvas (or, for the gradient pieces, pure DOM). Kiosk-friendly by
// construction: ~30fps cap, `requestAnimationFrame` (a dark screen costs zero),
// pause when the document is hidden, honour `prefers-reduced-motion`, no CSS
// `filter` blurs (the wall tablets can't afford them). Every layer is
// `pointerEvents: none` — the board stays fully interactive above it.
//
// Palette: presets sample the live theme tokens (`--bf-*`) at mount so they
// re-skin with the theme; scene-inherent hues (a dawn sky, rain) are the same
// deliberate carve-out as the Effects tiles' literal gradients.
// ─────────────────────────────────────────────────────────────────────────────

import React, { useEffect, useMemo, useRef, useState } from "react";

/** Per-board background spec, stored verbatim in `dashboards.background`. */
export interface BoardBackgroundCfg {
  kind: "preset" | "upload";
  /** Preset id (see BACKGROUND_PRESETS). */
  preset?: string;
  /** 0–0.85 darkening overlay for widget legibility. */
  scrim?: number;
  /** Animation speed multiplier (0.25–2.5, default 1). */
  speed?: number;
  /** false = render a still frame (battery paranoia / taste). */
  animate?: boolean;
  /** Upload: media mime (video mimes render a <video> loop). */
  mime?: string;
  /** Upload: cache-buster stamped on each replace. */
  v?: number;
}

export const BACKGROUND_PRESETS: { id: string; label: string; hint: string }[] = [
  { id: "synthwave", label: "Synthwave horizon", hint: "Neon grid terrain rolling toward you — ridges, valleys, turns" },
  { id: "filigree", label: "Circuit filigree", hint: "Engraved trace-work with pulses of light travelling the paths" },
  { id: "astrolabe", label: "Astrolabe night", hint: "Slow stars, faint constellations, a great rotating ring" },
  { id: "embers", label: "Ember drift", hint: "Sparse gold and violet motes rising in the dark" },
  { id: "aurora", label: "Aurora veil", hint: "Gradient curtains breathing across the upper sky" },
  { id: "sky", label: "Day & night sky", hint: "A muted sky that tracks the real time of day" },
  { id: "weather", label: "Weather-lit", hint: "Rain, snow, or drifting cloud from the board's weather widget" },
];

// ── Palette helpers ───────────────────────────────────────────────────────────

/** Resolve a live theme token to a concrete hex (canvas can't stroke a var()). */
function themeHex(name: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  const v = getComputedStyle(document.documentElement).getPropertyValue(`--bf-${name}`).trim();
  return v || fallback;
}

/** #rgb / #rrggbb → rgba() with the given alpha (pass-through otherwise). */
function hexA(hex: string, a: number): string {
  const m = hex.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (!m) return hex;
  let s = m[1];
  if (s.length === 3) s = s.split("").map((c) => c + c).join("");
  const n = parseInt(s, 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}

/** Linear-interpolate two hex colours (for the sky's hour palette). */
function lerpHex(a: string, b: string, t: number): string {
  const pa = a.match(/^#([0-9a-f]{6})$/i), pb = b.match(/^#([0-9a-f]{6})$/i);
  if (!pa || !pb) return a;
  const na = parseInt(pa[1], 16), nb = parseInt(pb[1], 16);
  const ch = (sh: number) => Math.round(((na >> sh) & 255) + (((nb >> sh) & 255) - ((na >> sh) & 255)) * t);
  return `rgb(${ch(16)},${ch(8)},${ch(0)})`;
}

interface Palette {
  void_: string;
  cyan: string;
  violet: string;
  gold: string;
  goldBright: string;
  rose: string;
}

function livePalette(): Palette {
  return {
    void_: themeHex("void", "#07060b"),
    cyan: themeHex("cyan", "#41e0e6"),
    violet: themeHex("violet", "#9a6ce8"),
    gold: themeHex("gold", "#c9a45c"),
    goldBright: themeHex("goldBright", "#ffd98a"),
    rose: themeHex("rose", "#e06a8a"),
  };
}

// ── Deterministic noise / rng ─────────────────────────────────────────────────

/** Hash noise in world coordinates — stable as the camera scrolls. */
function h2(x: number, y: number): number {
  const n = Math.sin(x * 127.1 + y * 311.7) * 43758.5453;
  return n - Math.floor(n);
}

/** Smooth 2D value noise (0..1). */
function vnoise(x: number, y: number): number {
  const xi = Math.floor(x), yi = Math.floor(y);
  const xf = x - xi, yf = y - yi;
  const u = xf * xf * (3 - 2 * xf), v = yf * yf * (3 - 2 * yf);
  return (
    h2(xi, yi) * (1 - u) * (1 - v) +
    h2(xi + 1, yi) * u * (1 - v) +
    h2(xi, yi + 1) * (1 - u) * v +
    h2(xi + 1, yi + 1) * u * v
  );
}

/** Seeded rng (mulberry32) so generated scenes are stable per mount/size. */
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ── Animated-canvas plumbing ──────────────────────────────────────────────────

/** A preset's frame renderer: `t` is speed-scaled seconds since mount. */
type DrawFn = (ctx: CanvasRenderingContext2D, w: number, h: number, t: number, dt: number) => void;
/** Factory: build a renderer (with its own particle state) for a palette. */
type PresetFactory = (p: Palette, extra: { weather?: string | null }) => DrawFn;

const FRAME_MS = 33; // ~30fps — plenty for atmosphere, kind to the tablets

function PresetCanvas({ make, weather, speed, animate }: { make: PresetFactory; weather?: string | null; speed: number; animate: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  // Live-tunable: the frame loop reads speed from a ref, so dragging the Speed
  // slider retunes the scene in place instead of tearing the canvas down (which
  // flashed a black frame per tick).
  const speedRef = useRef(speed);
  speedRef.current = speed;
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const draw = make(livePalette(), { weather });
    let raf = 0;
    let last = 0;
    let t = 0;
    let cssW = 0;
    let cssH = 0;
    const still = !animate || window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    const resize = () => {
      const r = canvas.getBoundingClientRect();
      // Cap the backing store at 1.5× — a background doesn't need retina text
      // sharpness and the fill-rate savings matter on the kiosks.
      const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
      cssW = Math.max(1, r.width);
      cssH = Math.max(1, r.height);
      canvas.width = Math.round(cssW * dpr);
      canvas.height = Math.round(cssH * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      // Seed a still with a mid-scene time so it doesn't look like frame zero.
      if (still) draw(ctx, cssW, cssH, 20, 0);
    };
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    resize();

    const frame = (now: number) => {
      raf = requestAnimationFrame(frame);
      if (now - last < FRAME_MS) return;
      const dt = (last === 0 ? 0 : Math.min(0.1, (now - last) / 1000)) * speedRef.current;
      last = now;
      t += dt;
      draw(ctx, cssW, cssH, t, dt);
    };
    const start = () => { if (!raf && !still) raf = requestAnimationFrame(frame); };
    const stop = () => { cancelAnimationFrame(raf); raf = 0; last = 0; };
    const onVis = () => (document.hidden ? stop() : start());
    document.addEventListener("visibilitychange", onVis);
    start();
    return () => { stop(); ro.disconnect(); document.removeEventListener("visibilitychange", onVis); };
  }, [make, weather, animate]);
  return <canvas ref={ref} style={{ position: "absolute", inset: 0, width: "100%", height: "100%" }} />;
}

// ── Preset: synthwave horizon ─────────────────────────────────────────────────

const makeSynthwave: PresetFactory = (p) => {
  // Camera path: the valley the "road" follows drifts with slow noise — the
  // grid rises into ridges at the sides and banks into turns, never repeating.
  const ROWS = 26;
  const XS: number[] = [];
  for (let x = -9; x <= 9; x++) XS.push(x);
  const curve = (z: number) => (vnoise(z * 0.055, 7.3) - 0.5) * 5.2;
  const height = (x: number, z: number) => {
    const c = curve(z);
    const side = Math.min(1, Math.max(0, (Math.abs(x - c) - 1.1) / 2.6)); // flat valley, tall flanks
    const n = vnoise(x * 0.42, z * 0.3) * 0.75 + vnoise(x * 0.13, z * 0.09) * 0.45;
    return n * side;
  };
  return (ctx, w, h, t) => {
    const horizon = h * 0.42;
    const camZ = t * 2.2;
    // Sky
    const sky = ctx.createLinearGradient(0, 0, 0, horizon);
    sky.addColorStop(0, p.void_);
    sky.addColorStop(0.75, hexA(p.violet, 0.14));
    sky.addColorStop(1, hexA(p.rose, 0.2));
    ctx.fillStyle = p.void_;
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, w, horizon);
    // Sun: gold disc sliced by widening sky bands (the classic).
    const sunR = Math.min(w, h) * 0.24;
    const sunY = horizon - sunR * 0.15;
    ctx.save();
    ctx.beginPath();
    ctx.arc(w / 2, sunY, sunR, 0, Math.PI * 2);
    ctx.clip();
    const sun = ctx.createLinearGradient(0, sunY - sunR, 0, sunY + sunR);
    sun.addColorStop(0, p.goldBright);
    sun.addColorStop(0.55, p.gold);
    sun.addColorStop(1, p.rose);
    ctx.fillStyle = sun;
    ctx.fillRect(w / 2 - sunR, sunY - sunR, sunR * 2, sunR * 2);
    ctx.fillStyle = p.void_;
    for (let i = 0; i < 6; i++) {
      const yy = sunY + sunR * (0.05 + i * 0.16) - ((t * 6) % (sunR * 0.16));
      ctx.fillRect(w / 2 - sunR, yy, sunR * 2, 1.5 + i * 1.9);
    }
    ctx.restore();
    // Horizon glow
    const hg = ctx.createLinearGradient(0, horizon - 14, 0, horizon + 22);
    hg.addColorStop(0, "transparent");
    hg.addColorStop(0.45, hexA(p.cyan, 0.5));
    hg.addColorStop(1, "transparent");
    ctx.fillStyle = hg;
    ctx.fillRect(0, horizon - 14, w, 36);
    // Ground fill below horizon
    ctx.fillStyle = hexA(p.violet, 0.06);
    ctx.fillRect(0, horizon, w, h - horizon);
    // Terrain projection
    const spread = w * 0.62;
    const K = (h - horizon) * 0.98;
    const amp = h * 0.5;
    const px = (x: number, zRel: number) => w / 2 + (x * spread) / zRel;
    const py = (x: number, z: number, zRel: number) => horizon + K / zRel - (height(x, z) * amp) / zRel;
    const base = Math.floor(camZ);
    const rows: { zRel: number; z: number }[] = [];
    for (let i = 1; i <= ROWS; i++) {
      const z = base + i;
      rows.push({ z, zRel: z - camZ });
    }
    ctx.lineJoin = "round";
    // Two passes: a soft wide glow, then the bright line. Latitudes…
    for (const pass of [0, 1]) {
      for (const { z, zRel } of rows) {
        const a = Math.min(0.85, 1.6 / zRel) * (pass ? 1 : 0.35);
        ctx.strokeStyle = pass ? hexA(p.violet, a) : hexA(p.violet, a * 0.6);
        ctx.lineWidth = pass ? 1 : 3;
        ctx.beginPath();
        for (let j = 0; j < XS.length; j++) {
          const x = XS[j];
          const X = px(x, zRel), Y = py(x, z, zRel);
          if (j === 0) ctx.moveTo(X, Y);
          else ctx.lineTo(X, Y);
        }
        ctx.stroke();
      }
      // …then longitudes.
      for (const x of XS) {
        const a = (Math.abs(x) < 2 ? 0.55 : 0.4) * (pass ? 1 : 0.35);
        ctx.strokeStyle = pass ? hexA(x === 0 ? p.cyan : p.violet, a) : hexA(p.violet, a * 0.6);
        ctx.lineWidth = pass ? 1 : 3;
        ctx.beginPath();
        let first = true;
        for (const { z, zRel } of rows) {
          const X = px(x, zRel), Y = py(x, z, zRel);
          if (first) { ctx.moveTo(X, Y); first = false; }
          else ctx.lineTo(X, Y);
        }
        ctx.stroke();
      }
    }
    // Fade the far field into the horizon glow.
    const fade = ctx.createLinearGradient(0, horizon, 0, horizon + h * 0.12);
    fade.addColorStop(0, hexA(p.void_, 0.85));
    fade.addColorStop(1, "transparent");
    ctx.fillStyle = fade;
    ctx.fillRect(0, horizon, w, h * 0.12);
  };
};

// ── Preset: circuit filigree ──────────────────────────────────────────────────

const makeFiligree: PresetFactory = (p) => {
  type Trace = { pts: { x: number; y: number }[]; lens: number[]; total: number };
  let traces: Trace[] = [];
  let sized = "";
  let off: HTMLCanvasElement | null = null;
  type Pulse = { ti: number; pos: number; speed: number };
  let pulses: Pulse[] = [];
  let nextPulse = 0;

  const build = (w: number, h: number) => {
    const r = rng(Math.round(w * 7919 + h));
    const g = Math.max(36, Math.round(Math.min(w, h) / 14));
    const dirs = [
      [1, 0], [-1, 0], [0, 1], [0, -1],
      [1, 1], [1, -1], [-1, 1], [-1, -1],
    ];
    traces = [];
    for (let i = 0; i < Math.round((w * h) / 42000) + 14; i++) {
      const pts = [{ x: Math.round((r() * w) / g) * g, y: Math.round((r() * h) / g) * g }];
      let d = dirs[Math.floor(r() * dirs.length)];
      const steps = 5 + Math.floor(r() * 9);
      for (let s = 0; s < steps; s++) {
        if (r() < 0.45) {
          const cand = dirs.filter(([dx, dy]) => !(dx === -d[0] && dy === -d[1]));
          d = cand[Math.floor(r() * cand.length)];
        }
        const last = pts[pts.length - 1];
        const step = g * (1 + Math.floor(r() * 2));
        pts.push({ x: last.x + d[0] * step, y: last.y + d[1] * step });
      }
      const lens = [0];
      let total = 0;
      for (let j = 1; j < pts.length; j++) {
        total += Math.hypot(pts[j].x - pts[j - 1].x, pts[j].y - pts[j - 1].y);
        lens.push(total);
      }
      traces.push({ pts, lens, total });
    }
    // Engrave the static layer once.
    off = document.createElement("canvas");
    off.width = Math.max(1, Math.round(w));
    off.height = Math.max(1, Math.round(h));
    const c = off.getContext("2d")!;
    c.fillStyle = p.void_;
    c.fillRect(0, 0, w, h);
    c.strokeStyle = hexA(p.gold, 0.1);
    c.lineWidth = 1;
    for (const tr of traces) {
      c.beginPath();
      tr.pts.forEach((pt, j) => (j ? c.lineTo(pt.x, pt.y) : c.moveTo(pt.x, pt.y)));
      c.stroke();
      // Terminal "vias": small diamonds, the filigree vocabulary.
      for (const pt of [tr.pts[0], tr.pts[tr.pts.length - 1]]) {
        c.save();
        c.translate(pt.x, pt.y);
        c.rotate(Math.PI / 4);
        c.strokeStyle = hexA(p.gold, 0.16);
        c.strokeRect(-2.5, -2.5, 5, 5);
        c.restore();
        c.strokeStyle = hexA(p.gold, 0.1);
      }
    }
    pulses = [];
  };

  const pointAt = (tr: Trace, d: number) => {
    let j = 1;
    while (j < tr.lens.length - 1 && tr.lens[j] < d) j++;
    const seg = tr.lens[j] - tr.lens[j - 1] || 1;
    const f = (d - tr.lens[j - 1]) / seg;
    return {
      x: tr.pts[j - 1].x + (tr.pts[j].x - tr.pts[j - 1].x) * f,
      y: tr.pts[j - 1].y + (tr.pts[j].y - tr.pts[j - 1].y) * f,
    };
  };

  return (ctx, w, h, t, dt) => {
    const key = `${Math.round(w)}x${Math.round(h)}`;
    if (key !== sized) { sized = key; build(w, h); }
    if (off) ctx.drawImage(off, 0, 0, w, h);
    if (t >= nextPulse && pulses.length < 3 && traces.length) {
      pulses.push({ ti: Math.floor(h2(t, 1.7) * traces.length) % traces.length, pos: 0, speed: 130 + h2(t, 9.2) * 160 });
      nextPulse = t + 1.6 + h2(t, 3.3) * 2.6;
    }
    pulses = pulses.filter((pu) => {
      const tr = traces[pu.ti];
      pu.pos += pu.speed * (dt || 1 / 30);
      if (pu.pos > tr.total + 60) return false;
      // Trailing light: the last ~70px of path behind the pulse head.
      const head = Math.min(pu.pos, tr.total);
      const tail = Math.max(0, pu.pos - 70);
      const steps = 7;
      for (let s = 0; s < steps; s++) {
        const d0 = tail + ((head - tail) * s) / steps;
        const d1 = tail + ((head - tail) * (s + 1)) / steps;
        const a = (s + 1) / steps;
        const p0 = pointAt(tr, d0), p1 = pointAt(tr, d1);
        ctx.strokeStyle = hexA(p.goldBright, 0.55 * a);
        ctx.lineWidth = 1.4;
        ctx.beginPath();
        ctx.moveTo(p0.x, p0.y);
        ctx.lineTo(p1.x, p1.y);
        ctx.stroke();
      }
      if (pu.pos <= tr.total) {
        const hp = pointAt(tr, head);
        const gl = ctx.createRadialGradient(hp.x, hp.y, 0, hp.x, hp.y, 9);
        gl.addColorStop(0, hexA(p.goldBright, 0.9));
        gl.addColorStop(1, "transparent");
        ctx.fillStyle = gl;
        ctx.fillRect(hp.x - 9, hp.y - 9, 18, 18);
      }
      return true;
    });
  };
};

// ── Preset: astrolabe night ───────────────────────────────────────────────────

const makeAstrolabe: PresetFactory = (p) => {
  type Star = { x: number; y: number; r: number; ph: number; f: number; gold: boolean };
  let stars: Star[] = [];
  let cons: { x: number; y: number }[][] = [];
  let sized = "";
  let shoot: { x: number; y: number; vx: number; vy: number; born: number } | null = null;
  let nextShoot = 6;

  const build = (w: number, h: number) => {
    const r = rng(Math.round(w * 31 + h * 7));
    stars = [];
    const n = Math.round((w * h) / 8500);
    for (let i = 0; i < n; i++) {
      stars.push({
        x: r() * w, y: r() * h,
        r: 0.4 + r() * 1.1,
        ph: r() * Math.PI * 2,
        f: 0.3 + r() * 1.1,
        gold: r() < 0.18,
      });
    }
    cons = [];
    for (let c = 0; c < 4; c++) {
      const chain: { x: number; y: number }[] = [];
      let x = r() * w, y = r() * h * 0.7;
      for (let s = 0; s < 4 + Math.floor(r() * 3); s++) {
        chain.push({ x, y });
        x += (r() - 0.5) * w * 0.16;
        y += (r() - 0.5) * h * 0.2;
      }
      cons.push(chain);
    }
  };

  return (ctx, w, h, t) => {
    const key = `${Math.round(w)}x${Math.round(h)}`;
    if (key !== sized) { sized = key; build(w, h); }
    ctx.fillStyle = p.void_;
    ctx.fillRect(0, 0, w, h);
    // Constellations: hairline gold, engraved.
    ctx.strokeStyle = hexA(p.gold, 0.1);
    ctx.lineWidth = 1;
    for (const chain of cons) {
      ctx.beginPath();
      chain.forEach((pt, j) => (j ? ctx.lineTo(pt.x, pt.y) : ctx.moveTo(pt.x, pt.y)));
      ctx.stroke();
    }
    // Stars twinkle on individual phases.
    for (const s of stars) {
      const a = Math.max(0.06, 0.3 + 0.4 * Math.sin(t * s.f + s.ph));
      ctx.fillStyle = s.gold ? hexA(p.gold, a) : `rgba(214,225,240,${a})`;
      ctx.fillRect(s.x, s.y, s.r, s.r);
    }
    // The great ring: partially off-canvas, one revolution ≈ an hour.
    const cx = w * 0.86, cy = h * 0.18, R = Math.min(w, h) * 0.52;
    ctx.save();
    ctx.translate(cx, cy);
    ctx.rotate((t / 3600) * Math.PI * 2 + 0.4);
    ctx.strokeStyle = hexA(p.gold, 0.13);
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.arc(0, 0, R, 0, Math.PI * 2); ctx.stroke();
    ctx.beginPath(); ctx.arc(0, 0, R * 0.86, 0, Math.PI * 2); ctx.stroke();
    for (let i = 0; i < 72; i++) {
      const ang = (i / 72) * Math.PI * 2;
      const long = i % 6 === 0;
      const r0 = R * (long ? 0.86 : 0.92);
      ctx.strokeStyle = hexA(p.gold, long ? 0.2 : 0.11);
      ctx.beginPath();
      ctx.moveTo(Math.cos(ang) * r0, Math.sin(ang) * r0);
      ctx.lineTo(Math.cos(ang) * R, Math.sin(ang) * R);
      ctx.stroke();
    }
    ctx.restore();
    // Occasional shooting star.
    if (!shoot && t > nextShoot) {
      const r = h2(t, 5.5);
      shoot = { x: w * (0.15 + r * 0.6), y: h * 0.08, vx: (0.5 + r) * w * 0.5, vy: h * 0.35, born: t };
    }
    if (shoot) {
      const age = t - shoot.born;
      if (age > 0.9) { shoot = null; nextShoot = t + 7 + h2(t, 2.2) * 16; }
      else {
        const x = shoot.x + shoot.vx * age, y = shoot.y + shoot.vy * age;
        const a = age < 0.15 ? age / 0.15 : 1 - (age - 0.15) / 0.75;
        const g = ctx.createLinearGradient(x - shoot.vx * 0.12, y - shoot.vy * 0.12, x, y);
        g.addColorStop(0, "transparent");
        g.addColorStop(1, `rgba(220,232,245,${0.8 * a})`);
        ctx.strokeStyle = g;
        ctx.lineWidth = 1.4;
        ctx.beginPath();
        ctx.moveTo(x - shoot.vx * 0.12, y - shoot.vy * 0.12);
        ctx.lineTo(x, y);
        ctx.stroke();
      }
    }
  };
};

// ── Preset: ember drift ───────────────────────────────────────────────────────

const makeEmbers: PresetFactory = (p) => {
  type Ember = { x: number; y: number; r: number; vy: number; wf: number; wa: number; ph: number; violet: boolean; a: number };
  let embers: Ember[] = [];
  let sized = "";
  const build = (w: number, h: number) => {
    const r = rng(Math.round(w + h * 13));
    embers = [];
    for (let i = 0; i < Math.round(w / 26); i++) {
      embers.push({
        x: r() * w, y: r() * h,
        r: 0.8 + r() * 1.8,
        vy: 9 + r() * 20,
        wf: 0.4 + r() * 0.9,
        wa: 6 + r() * 18,
        ph: r() * Math.PI * 2,
        violet: r() < 0.3,
        a: 0.35 + r() * 0.5,
      });
    }
  };
  return (ctx, w, h, t, dt) => {
    const key = `${Math.round(w)}x${Math.round(h)}`;
    if (key !== sized) { sized = key; build(w, h); }
    ctx.fillStyle = p.void_;
    ctx.fillRect(0, 0, w, h);
    for (const e of embers) {
      e.y -= e.vy * dt;
      if (e.y < -8) { e.y = h + 8; e.x = h2(e.ph, t) * w; }
      const x = e.x + Math.sin(t * e.wf + e.ph) * e.wa;
      const fade = Math.min(1, e.y / (h * 0.3)); // dim as they near the top
      const col = e.violet ? p.violet : p.gold;
      const g = ctx.createRadialGradient(x, e.y, 0, x, e.y, e.r * 4);
      g.addColorStop(0, hexA(col, e.a * fade));
      g.addColorStop(1, "transparent");
      ctx.fillStyle = g;
      ctx.fillRect(x - e.r * 4, e.y - e.r * 4, e.r * 8, e.r * 8);
    }
  };
};

// ── Preset: weather-lit ───────────────────────────────────────────────────────

const makeWeather: PresetFactory = (p, { weather }) => {
  const cond = (weather ?? "").toLowerCase();
  const rainy = /rain|pour|hail|lightning-rainy/.test(cond);
  const pouring = /pour/.test(cond);
  const lightning = /lightning/.test(cond);
  const snowy = /snow/.test(cond);
  const foggy = /fog|mist|haze/.test(cond);
  const cloudy = /cloud|wind|overcast/.test(cond);
  const clearNight = /clear-night/.test(cond);
  const sunny = /sunny|clear$/.test(cond) && !clearNight;

  type Drop = { x: number; y: number; v: number; l: number; drift: number };
  type Blob = { x: number; y: number; r: number; v: number; a: number };
  let drops: Drop[] = [];
  let blobs: Blob[] = [];
  let stars: { x: number; y: number; ph: number; f: number }[] = [];
  let sized = "";
  let flashAt = 4;
  let flash = 0;

  const build = (w: number, h: number) => {
    const r = rng(Math.round(w * 3 + h));
    drops = [];
    const n = snowy ? Math.round(w / 12) : Math.round(w / (pouring ? 4 : 8));
    for (let i = 0; i < n; i++) {
      drops.push({
        x: r() * w, y: r() * h,
        v: snowy ? 24 + r() * 40 : 420 + r() * 380,
        l: snowy ? 1.2 + r() * 1.8 : 10 + r() * 14,
        drift: snowy ? (r() - 0.5) * 30 : 20 + r() * 30,
      });
    }
    blobs = [];
    for (let i = 0; i < 5; i++) {
      blobs.push({ x: r() * w, y: r() * h * (foggy ? 1 : 0.5), r: w * (0.2 + r() * 0.22), v: 4 + r() * 9, a: 0.1 + r() * 0.1 });
    }
    stars = [];
    for (let i = 0; i < Math.round((w * h) / 16000); i++) {
      stars.push({ x: r() * w, y: r() * h, ph: r() * 6.28, f: 0.3 + r() });
    }
  };

  return (ctx, w, h, t, dt) => {
    const key = `${Math.round(w)}x${Math.round(h)}`;
    if (key !== sized) { sized = key; build(w, h); }
    ctx.fillStyle = p.void_;
    ctx.fillRect(0, 0, w, h);
    if (clearNight || (!rainy && !snowy && !foggy && !cloudy && !sunny)) {
      // Clear night (also the no-data fallback): sparse calm stars.
      for (const s of stars) {
        const a = Math.max(0.05, 0.25 + 0.3 * Math.sin(t * s.f + s.ph));
        ctx.fillStyle = `rgba(210,222,238,${a})`;
        ctx.fillRect(s.x, s.y, 1, 1);
      }
    }
    if (sunny) {
      // Quiet gold rays from the top corner, barely-there.
      const g = ctx.createRadialGradient(w * 0.12, -h * 0.1, 0, w * 0.12, -h * 0.1, w * 0.9);
      g.addColorStop(0, hexA(p.gold, 0.16));
      g.addColorStop(0.5, hexA(p.gold, 0.04));
      g.addColorStop(1, "transparent");
      ctx.fillStyle = g;
      ctx.fillRect(0, 0, w, h);
      ctx.save();
      ctx.translate(w * 0.12, -h * 0.1);
      ctx.rotate(Math.sin(t * 0.05) * 0.02);
      for (let i = 0; i < 5; i++) {
        const ang = 0.5 + i * 0.22;
        const rg = ctx.createLinearGradient(0, 0, Math.cos(ang) * w, Math.sin(ang) * w);
        rg.addColorStop(0, hexA(p.goldBright, 0.07));
        rg.addColorStop(1, "transparent");
        ctx.strokeStyle = rg;
        ctx.lineWidth = 30;
        ctx.beginPath();
        ctx.moveTo(0, 0);
        ctx.lineTo(Math.cos(ang) * w * 1.4, Math.sin(ang) * w * 1.4);
        ctx.stroke();
      }
      ctx.restore();
    }
    if (cloudy || foggy) {
      for (const b of blobs) {
        b.x += b.v * dt;
        if (b.x - b.r > w) b.x = -b.r;
        const g = ctx.createRadialGradient(b.x, b.y, 0, b.x, b.y, b.r);
        g.addColorStop(0, `rgba(16,20,30,${b.a + 0.08})`);
        g.addColorStop(0.6, `rgba(120,132,152,${b.a * 0.35})`);
        g.addColorStop(1, "transparent");
        ctx.fillStyle = g;
        ctx.fillRect(b.x - b.r, b.y - b.r, b.r * 2, b.r * 2);
      }
    }
    if (rainy || snowy) {
      for (const d of drops) {
        d.y += d.v * dt;
        d.x += d.drift * dt;
        if (d.y > h + 20) { d.y = -20; d.x = h2(d.l, t) * w; }
        if (d.x > w + 10) d.x = -10;
        if (d.x < -10) d.x = w + 10;
        if (snowy) {
          ctx.fillStyle = "rgba(224,232,244,0.5)";
          ctx.beginPath();
          ctx.arc(d.x + Math.sin(t + d.l * 9) * 6, d.y, d.l, 0, Math.PI * 2);
          ctx.fill();
        } else {
          ctx.strokeStyle = "rgba(150,175,220,0.32)";
          ctx.lineWidth = 1;
          ctx.beginPath();
          ctx.moveTo(d.x, d.y);
          ctx.lineTo(d.x - d.drift * 0.03 * d.l, d.y - d.l);
          ctx.stroke();
        }
      }
    }
    if (lightning) {
      if (t > flashAt) { flash = 1; flashAt = t + 4 + h2(t, 8.8) * 12; }
      if (flash > 0) {
        // A double-blink: bright, dip, bright, decay.
        const a = flash > 0.75 ? 0.1 : flash > 0.6 ? 0.02 : flash > 0.45 ? 0.08 : flash * 0.1;
        ctx.fillStyle = `rgba(210,222,255,${a})`;
        ctx.fillRect(0, 0, w, h);
        flash = Math.max(0, flash - dt * 2.4);
      }
    }
  };
};

// ── Presets: aurora veil + day/night sky (pure DOM) ──────────────────────────

function AuroraLayer({ speed, animate }: { speed: number; animate: boolean }) {
  const p = useMemo(livePalette, []);
  const still = !animate || (typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const ribbon = (grad: string, top: string, height: string, anim: string, dur: number): React.CSSProperties => ({
    position: "absolute",
    left: "-25%",
    width: "150%",
    top,
    height,
    background: grad,
    borderRadius: "50%",
    animation: still ? "none" : `${anim} ${dur / speed}s ease-in-out infinite alternate`,
  });
  return (
    <div style={{ position: "absolute", inset: 0, background: p.void_ }}>
      <div style={ribbon(
        `linear-gradient(104deg, transparent 12%, ${hexA(p.cyan, 0.05)} 30%, ${hexA(p.cyan, 0.14)} 44%, ${hexA(p.violet, 0.1)} 60%, transparent 82%)`,
        "-12%", "52%", "bf-aurora-a", 26,
      )} />
      <div style={ribbon(
        `linear-gradient(96deg, transparent 18%, ${hexA(p.violet, 0.05)} 36%, ${hexA(p.violet, 0.13)} 52%, ${hexA(p.cyan, 0.07)} 68%, transparent 86%)`,
        "-4%", "44%", "bf-aurora-b", 34,
      )} />
      <div style={ribbon(
        `linear-gradient(110deg, transparent 8%, ${hexA(p.gold, 0.035)} 40%, ${hexA(p.cyan, 0.08)} 62%, transparent 88%)`,
        "6%", "38%", "bf-aurora-c", 44,
      )} />
    </div>
  );
}

/** Muted sky palette keyed by hour — deliberately dark-leaning so daytime never
 * floods a wall of dark widgets with white (scene-inherent hues, like Effects). */
const SKY_STOPS: [number, string, string][] = [
  [0, "#070a18", "#0c1024"],
  [4.5, "#0b0e24", "#191333"],
  [6, "#1c1a42", "#5c2e4e"],
  [7.5, "#243057", "#a05a52"],
  [10, "#20395e", "#39558a"],
  [14, "#274468", "#3d6291"],
  [17, "#2c3a63", "#84495a"],
  [19, "#251d4a", "#8a4438"],
  [20.5, "#131233", "#2c1c44"],
  [22, "#0a0d1f", "#141130"],
  [24, "#070a18", "#0c1024"],
];

function SkyLayer() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const iv = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(iv);
  }, []);
  const p = useMemo(livePalette, []);
  const hr = now.getHours() + now.getMinutes() / 60;
  let i = 0;
  while (i < SKY_STOPS.length - 2 && SKY_STOPS[i + 1][0] <= hr) i++;
  const [h0, t0, b0] = SKY_STOPS[i];
  const [h1, t1, b1] = SKY_STOPS[i + 1];
  const f = Math.min(1, Math.max(0, (hr - h0) / (h1 - h0 || 1)));
  const top = lerpHex(t0, t1, f);
  const bottom = lerpHex(b0, b1, f);
  // Sun (06–18) or moon glow tracking its arc across the board.
  const day = hr >= 6 && hr < 18;
  const arc = day ? (hr - 6) / 12 : ((hr >= 18 ? hr - 18 : hr + 6) / 12);
  const gx = 10 + arc * 80;
  const gy = 62 - Math.sin(arc * Math.PI) * 46;
  const glow = day ? hexA(p.gold, 0.22) : hexA(p.cyan, 0.13);
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        background: `radial-gradient(circle at ${gx}% ${gy}%, ${glow}, transparent 26%), linear-gradient(180deg, ${top}, ${bottom})`,
        transition: "background 2s linear",
      }}
    />
  );
}

// ── The component ─────────────────────────────────────────────────────────────

const PRESET_FACTORIES: Record<string, PresetFactory> = {
  synthwave: makeSynthwave,
  filigree: makeFiligree,
  astrolabe: makeAstrolabe,
  embers: makeEmbers,
  weather: makeWeather,
};

/** The background layer for one board: absolute-fills its (positioned) parent,
 * clipped to the canvas radius, always behind and inert to the widgets. */
export function BoardBackground({
  cfg,
  boardId,
  weather,
  radius = 0,
}: {
  cfg: BoardBackgroundCfg;
  boardId: string;
  /** Current weather condition (from the board's weather widget), for `weather`. */
  weather?: string | null;
  radius?: number | string;
}) {
  const speed = Math.min(2.5, Math.max(0.25, cfg.speed ?? 1));
  const animate = cfg.animate !== false;
  const scrim = Math.min(0.85, Math.max(0, cfg.scrim ?? 0));

  let layer: React.ReactNode = null;
  if (cfg.kind === "upload") {
    const src = `/api/dashboards/${boardId}/background/media?v=${cfg.v ?? 0}`;
    const fit: React.CSSProperties = { position: "absolute", inset: 0, width: "100%", height: "100%", objectFit: "cover" };
    layer = cfg.mime?.startsWith("video/") ? (
      <video src={src} style={fit} autoPlay={animate} loop muted playsInline />
    ) : (
      <img src={src} style={fit} alt="" draggable={false} />
    );
  } else if (cfg.preset === "aurora") {
    layer = <AuroraLayer speed={speed} animate={animate} />;
  } else if (cfg.preset === "sky") {
    layer = <SkyLayer />;
  } else if (cfg.preset && PRESET_FACTORIES[cfg.preset]) {
    layer = <PresetCanvas make={PRESET_FACTORIES[cfg.preset]} weather={weather} speed={speed} animate={animate} />;
  }
  if (!layer) return null;

  return (
    <div
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        overflow: "hidden",
        borderRadius: radius,
        pointerEvents: "none",
      }}
    >
      {layer}
      {scrim > 0 && <div style={{ position: "absolute", inset: 0, background: `rgba(0,0,0,${scrim})` }} />}
    </div>
  );
}
