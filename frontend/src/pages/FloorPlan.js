import { jsx as _jsx, Fragment as _Fragment, jsxs as _jsxs } from "react/jsx-runtime";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPlan, getPlan, getPlans, putPlanLayout, putPlanLights, removePlan, setLightState, xyToRgb, } from "../api";
import { S } from "../styles";
const TOOLS = [
    { id: "view", label: "View", hint: "Click a light to toggle it. Drag to pan, scroll to zoom." },
    { id: "floor", label: "Floor", hint: "Click-drag to paint floor tiles." },
    { id: "wall", label: "Wall", hint: "Click-drag along tile boundaries to draw walls. Leave gaps for doors." },
    { id: "erase", label: "Erase", hint: "Click-drag to remove tiles and walls." },
    { id: "place", label: "Lights", hint: "Pick a light from the palette, then click a tile — near an edge wall-mounts it, the middle ceiling-mounts it. Click a placed light to remove it." },
];
const tileKey = (x, y) => `${x},${y}`;
const wallKey = (x, y, dir) => `${x},${y},${dir}`;
export function FloorPlanPage({ lights }) {
    const [plans, setPlans] = useState([]);
    const [planId, setPlanId] = useState("");
    const [plan, setPlan] = useState(null);
    // Editable copies of the plan layout.
    const [tiles, setTiles] = useState(new Set());
    const [walls, setWalls] = useState(new Set());
    const [placements, setPlacements] = useState([]);
    const [dirty, setDirty] = useState(false);
    const [tool, setTool] = useState("view");
    const [selectedLight, setSelectedLight] = useState("");
    const [popover, setPopover] = useState(null);
    const [toast, setToast] = useState("");
    // Live light states: start from the lights prop, patched by SSE + optimistic toggles.
    const [statesById, setStatesById] = useState(new Map());
    useEffect(() => {
        setStatesById((prev) => {
            const next = new Map(prev);
            for (const l of lights)
                if (l.last_state && !next.has(l.id))
                    next.set(l.id, l.last_state);
            return next;
        });
    }, [lights]);
    useEffect(() => {
        const es = new EventSource("/api/events");
        es.addEventListener("light_state", (raw) => {
            const { device_id, state } = JSON.parse(raw.data);
            const light = lights.find((l) => l.device_id === device_id);
            if (!light)
                return;
            setStatesById((prev) => new Map(prev).set(light.id, state));
        });
        es.onerror = () => { };
        return () => es.close();
    }, [lights]);
    async function loadPlans() {
        const list = await getPlans();
        setPlans(list);
        if (list.length > 0 && !list.some((p) => p.id === planId))
            setPlanId(list[0].id);
    }
    useEffect(() => { loadPlans(); }, []); // eslint-disable-line react-hooks/exhaustive-deps
    useEffect(() => {
        if (!planId) {
            setPlan(null);
            return;
        }
        getPlan(planId).then((p) => {
            setPlan(p);
            setTiles(new Set(p.tiles.map(([x, y]) => tileKey(x, y))));
            setWalls(new Set(p.walls.map((w) => wallKey(w.x, w.y, w.dir))));
            setPlacements(p.lights);
            setDirty(false);
            setPopover(null);
        });
    }, [planId]);
    function showToast(msg) {
        setToast(msg);
        setTimeout(() => setToast(""), 3000);
    }
    async function handleCreate() {
        const name = window.prompt("Plan name (e.g. Ground Floor):");
        if (!name?.trim())
            return;
        const dims = window.prompt("Size in feet, width x height (max 128):", "50x40");
        const m = dims?.match(/^\s*(\d+)\s*[x×]\s*(\d+)\s*$/);
        if (!m)
            return;
        const { id } = await createPlan(name.trim(), Number(m[1]), Number(m[2]));
        await loadPlans();
        setPlanId(id);
    }
    async function handleDelete() {
        if (!plan)
            return;
        if (!window.confirm(`Delete plan "${plan.name}"?`))
            return;
        await removePlan(plan.id);
        setPlanId("");
        await loadPlans();
    }
    async function handleSave() {
        if (!plan)
            return;
        const tileArr = [...tiles].map((k) => k.split(",").map(Number));
        const wallArr = [...walls].map((k) => {
            const [x, y, dir] = k.split(",");
            return { x: Number(x), y: Number(y), dir: dir };
        });
        try {
            await putPlanLayout(plan.id, tileArr, wallArr);
            await putPlanLights(plan.id, placements);
            setDirty(false);
            showToast("Saved.");
        }
        catch (e) {
            showToast(`Save failed: ${e instanceof Error ? e.message : e}`);
        }
    }
    async function toggleLight(lightId) {
        const current = statesById.get(lightId) ?? { on: false };
        const next = { ...current, on: !current.on };
        setStatesById((prev) => new Map(prev).set(lightId, next)); // optimistic
        await setLightState(lightId, next);
    }
    const placedIds = new Set(placements.map((p) => p.light_id));
    return (_jsxs("div", { style: { padding: "1.5rem 2rem" }, children: [_jsxs("div", { style: { display: "flex", alignItems: "center", gap: "0.6rem", marginBottom: "0.9rem", flexWrap: "wrap" }, children: [plans.map((p) => (_jsx("button", { onClick: () => setPlanId(p.id), style: { ...S.buttonGhost, ...(p.id === planId ? { borderColor: "#f90", color: "#f90" } : {}) }, children: p.name }, p.id))), _jsx("button", { onClick: handleCreate, style: S.buttonGhost, children: "+ New plan" }), plan && (_jsxs(_Fragment, { children: [_jsx("span", { style: { flex: 1 } }), _jsx("button", { onClick: handleSave, disabled: !dirty, style: dirty ? S.button : S.buttonGhost, children: dirty ? "Save changes" : "Saved" }), _jsx("button", { onClick: handleDelete, style: S.buttonDanger, children: "Delete" })] }))] }), toast && (_jsx("div", { style: { background: "#1e3a1e", border: "1px solid #2a5a2a", borderRadius: 8, padding: "0.5rem 1rem", marginBottom: "0.75rem", color: "#8f8", fontSize: "0.875rem" }, children: toast })), !plan ? (_jsx("p", { style: { color: "#666" }, children: "No floor plans yet. Create one, paint your layout, then place your lights on it." })) : (_jsxs(_Fragment, { children: [_jsxs("div", { style: { display: "flex", gap: "0.4rem", marginBottom: "0.5rem", alignItems: "center", flexWrap: "wrap" }, children: [TOOLS.map((t) => (_jsx("button", { onClick: () => { setTool(t.id); setPopover(null); }, style: { ...S.buttonGhost, ...(tool === t.id ? { borderColor: "#f90", color: "#f90" } : {}) }, children: t.label }, t.id))), _jsx("span", { style: { color: "#666", fontSize: "0.78rem", marginLeft: "0.5rem" }, children: TOOLS.find((t) => t.id === tool)?.hint })] }), _jsxs("div", { style: { display: "flex", gap: "1rem", alignItems: "flex-start" }, children: [_jsx(PlanCanvas, { plan: plan, tiles: tiles, walls: walls, placements: placements, statesById: statesById, tool: tool, selectedLight: selectedLight, onMutate: (fn) => { fn(); setDirty(true); setPopover(null); }, setTiles: setTiles, setWalls: setWalls, setPlacements: setPlacements, onLightClick: (pls, px, py) => {
                                    if (pls.length === 1)
                                        toggleLight(pls[0].light_id);
                                    else
                                        setPopover({ px, py, placements: pls });
                                } }), tool === "place" && (_jsxs("div", { style: { width: 200, flexShrink: 0 }, children: [_jsx("h3", { style: { margin: "0 0 0.5rem", fontSize: "0.9rem", color: "#aaa" }, children: "Lights" }), _jsxs("div", { style: { display: "flex", flexDirection: "column", gap: "0.3rem" }, children: [lights.map((l) => (_jsxs("button", { onClick: () => setSelectedLight(l.id === selectedLight ? "" : l.id), style: {
                                                    ...S.buttonGhost,
                                                    textAlign: "left",
                                                    fontSize: "0.8rem",
                                                    ...(l.id === selectedLight ? { borderColor: "#f90", color: "#f90" } : {}),
                                                    ...(placedIds.has(l.id) ? { opacity: 0.55 } : {}),
                                                }, children: [placedIds.has(l.id) ? "✓ " : "", l.name] }, l.id))), lights.length === 0 && (_jsx("span", { style: { color: "#666", fontSize: "0.8rem" }, children: "No lights discovered yet." }))] })] }))] }), popover && (_jsxs("div", { style: {
                            position: "fixed",
                            left: popover.px,
                            top: popover.py,
                            background: "#1c1c1c",
                            border: "1px solid #333",
                            borderRadius: 8,
                            padding: "0.5rem",
                            zIndex: 10,
                            display: "flex",
                            flexDirection: "column",
                            gap: "0.3rem",
                            boxShadow: "0 4px 16px rgba(0,0,0,0.5)",
                        }, children: [popover.placements.map((p) => {
                                const light = lights.find((l) => l.id === p.light_id);
                                const on = statesById.get(p.light_id)?.on ?? false;
                                return (_jsxs("button", { onClick: () => toggleLight(p.light_id), style: { ...S.buttonGhost, fontSize: "0.8rem", textAlign: "left", color: on ? "#f90" : "#888" }, children: [on ? "● " : "○ ", light?.name ?? p.light_id] }, p.light_id));
                            }), _jsx("button", { onClick: () => setPopover(null), style: { ...S.buttonGhost, fontSize: "0.75rem", color: "#666" }, children: "Close" })] }))] }))] }));
}
// ── Canvas ───────────────────────────────────────────────────────────────────
function PlanCanvas({ plan, tiles, walls, placements, statesById, tool, selectedLight, onMutate, setTiles, setWalls, setPlacements, onLightClick, }) {
    const canvasRef = useRef(null);
    const [view, setView] = useState({ cell: 0, ox: 0, oy: 0 }); // cell=0 → fit on first draw
    const drag = useRef(null);
    // Mount-point position within a tile, in tile units.
    const mountOffset = (m) => m === "n" ? [0.5, 0.08] : m === "s" ? [0.5, 0.92] : m === "e" ? [0.92, 0.5] : m === "w" ? [0.08, 0.5] : [0.5, 0.5];
    const draw = useCallback(() => {
        const canvas = canvasRef.current;
        if (!canvas)
            return;
        const dpr = window.devicePixelRatio || 1;
        const cssW = canvas.clientWidth;
        const cssH = canvas.clientHeight;
        // Round before comparing: with fractional DPR an int-vs-float comparison
        // is always unequal, and rewriting the width attribute every frame fed a
        // flexbox min-width:auto growth loop.
        const targetW = Math.round(cssW * dpr);
        const targetH = Math.round(cssH * dpr);
        if (canvas.width !== targetW || canvas.height !== targetH) {
            canvas.width = targetW;
            canvas.height = targetH;
        }
        let { cell, ox, oy } = view;
        if (cell === 0) {
            cell = Math.max(8, Math.min(48, Math.floor(Math.min(cssW / plan.width, cssH / plan.height))));
            ox = (cssW - plan.width * cell) / 2;
            oy = (cssH - plan.height * cell) / 2;
            setView({ cell, ox, oy });
            return;
        }
        const ctx = canvas.getContext("2d");
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx.clearRect(0, 0, cssW, cssH);
        ctx.fillStyle = "#0d0d0f";
        ctx.fillRect(0, 0, cssW, cssH);
        // Floor tiles
        ctx.fillStyle = "#1d2129";
        for (const k of tiles) {
            const [x, y] = k.split(",").map(Number);
            ctx.fillRect(ox + x * cell, oy + y * cell, cell, cell);
        }
        // Grid
        ctx.strokeStyle = "rgba(255,255,255,0.05)";
        ctx.lineWidth = 1;
        for (let x = 0; x <= plan.width; x++) {
            ctx.beginPath();
            ctx.moveTo(ox + x * cell, oy);
            ctx.lineTo(ox + x * cell, oy + plan.height * cell);
            ctx.stroke();
        }
        for (let y = 0; y <= plan.height; y++) {
            ctx.beginPath();
            ctx.moveTo(ox, oy + y * cell);
            ctx.lineTo(ox + plan.width * cell, oy + y * cell);
            ctx.stroke();
        }
        // Walls
        ctx.strokeStyle = "#d9deeb";
        ctx.lineWidth = Math.max(2, cell * 0.14);
        ctx.lineCap = "square";
        for (const k of walls) {
            const [xs, ys, dir] = k.split(",");
            const x = Number(xs), y = Number(ys);
            ctx.beginPath();
            if (dir === "h") {
                ctx.moveTo(ox + x * cell, oy + y * cell);
                ctx.lineTo(ox + (x + 1) * cell, oy + y * cell);
            }
            else {
                ctx.moveTo(ox + x * cell, oy + y * cell);
                ctx.lineTo(ox + x * cell, oy + (y + 1) * cell);
            }
            ctx.stroke();
        }
        // Lights, clustered by (x, y, mount)
        const clusters = new Map();
        for (const p of placements) {
            const k = `${p.x},${p.y},${p.mount}`;
            clusters.set(k, [...(clusters.get(k) ?? []), p]);
        }
        for (const group of clusters.values()) {
            const p = group[0];
            const [mx, my] = mountOffset(p.mount);
            const cx = ox + (p.x + mx) * cell;
            const cy = oy + (p.y + my) * cell;
            const r = Math.max(3.5, cell * 0.18);
            // Aggregate: lit if any member is on; color from the first lit member.
            const states = group.map((g) => statesById.get(g.light_id));
            const lit = states.find((s) => s?.on);
            let fill = "#3a3d45";
            if (lit) {
                if (lit.color) {
                    const [rr, gg, bb] = xyToRgb(lit.color.x, lit.color.y, Math.max(lit.color.brightness, 0.25));
                    fill = `rgb(${rr},${gg},${bb})`;
                }
                else {
                    fill = "#ffd9a0";
                }
                const glow = ctx.createRadialGradient(cx, cy, r * 0.5, cx, cy, r * 3);
                glow.addColorStop(0, fill);
                glow.addColorStop(1, "rgba(0,0,0,0)");
                ctx.globalAlpha = 0.35 * Math.min(1, ((lit.brightness ?? 100) / 100) + 0.3);
                ctx.fillStyle = glow;
                ctx.beginPath();
                ctx.arc(cx, cy, r * 3, 0, Math.PI * 2);
                ctx.fill();
                ctx.globalAlpha = 1;
            }
            ctx.fillStyle = fill;
            ctx.beginPath();
            ctx.arc(cx, cy, r, 0, Math.PI * 2);
            ctx.fill();
            ctx.strokeStyle = "rgba(0,0,0,0.6)";
            ctx.lineWidth = 1;
            ctx.stroke();
            if (group.length > 1) {
                ctx.fillStyle = "#fff";
                ctx.font = `${Math.max(9, cell * 0.3)}px system-ui`;
                ctx.textAlign = "left";
                ctx.textBaseline = "middle";
                ctx.fillText(`×${group.length}`, cx + r + 2, cy);
            }
        }
    }, [view, plan, tiles, walls, placements, statesById]);
    useEffect(() => { draw(); }, [draw]);
    useEffect(() => {
        // Redraw whenever the canvas box changes (window resize, palette
        // appearing/disappearing) — ResizeObserver catches both.
        const canvas = canvasRef.current;
        if (!canvas)
            return;
        const ro = new ResizeObserver(() => draw());
        ro.observe(canvas);
        return () => ro.disconnect();
    }, [draw]);
    // Reset the fit when switching plans.
    useEffect(() => { setView({ cell: 0, ox: 0, oy: 0 }); }, [plan.id]);
    function toGrid(e) {
        const rect = canvasRef.current.getBoundingClientRect();
        return {
            gx: (e.clientX - rect.left - view.ox) / view.cell,
            gy: (e.clientY - rect.top - view.oy) / view.cell,
        };
    }
    function nearestEdge(gx, gy) {
        const tx = Math.floor(gx), ty = Math.floor(gy);
        const fx = gx - tx, fy = gy - ty;
        const candidates = [
            { d: fy, x: tx, y: ty, dir: "h" },
            { d: 1 - fy, x: tx, y: ty + 1, dir: "h" },
            { d: fx, x: tx, y: ty, dir: "v" },
            { d: 1 - fx, x: tx + 1, y: ty, dir: "v" },
        ];
        candidates.sort((a, b) => a.d - b.d);
        const best = candidates[0];
        // Bounds: 'h' walls x<width, y<=height; 'v' walls x<=width, y<height.
        const xMax = best.dir === "v" ? plan.width : plan.width - 1;
        const yMax = best.dir === "h" ? plan.height : plan.height - 1;
        if (best.x < 0 || best.x > xMax || best.y < 0 || best.y > yMax)
            return null;
        return best;
    }
    function applyTool(gx, gy, e) {
        const tx = Math.floor(gx), ty = Math.floor(gy);
        const inBounds = tx >= 0 && tx < plan.width && ty >= 0 && ty < plan.height;
        if (tool === "floor" && inBounds) {
            onMutate(() => setTiles((prev) => new Set(prev).add(tileKey(tx, ty))));
        }
        else if (tool === "wall") {
            const edge = nearestEdge(gx, gy);
            if (edge)
                onMutate(() => setWalls((prev) => new Set(prev).add(wallKey(edge.x, edge.y, edge.dir))));
        }
        else if (tool === "erase") {
            const edge = nearestEdge(gx, gy);
            const nearWall = edge && Math.min(gx - Math.floor(gx), 1 - (gx - Math.floor(gx)), gy - Math.floor(gy), 1 - (gy - Math.floor(gy))) < 0.2;
            onMutate(() => {
                if (nearWall && edge)
                    setWalls((prev) => { const n = new Set(prev); n.delete(wallKey(edge.x, edge.y, edge.dir)); return n; });
                else if (inBounds)
                    setTiles((prev) => { const n = new Set(prev); n.delete(tileKey(tx, ty)); return n; });
            });
        }
        else if (tool === "place" && e.type === "pointerdown" && inBounds) {
            // Hit an existing placement? Remove it.
            const hit = hitPlacement(gx, gy);
            if (hit) {
                onMutate(() => setPlacements((prev) => prev.filter((p) => p.light_id !== hit.light_id)));
                return;
            }
            if (!selectedLight)
                return;
            const fx = gx - tx, fy = gy - ty;
            const m = Math.min(fx, 1 - fx, fy, 1 - fy);
            let mount = "c";
            if (m < 0.28) {
                mount = fy === m ? "n" : 1 - fy === m ? "s" : fx === m ? "w" : "e";
            }
            onMutate(() => setPlacements((prev) => [
                ...prev.filter((p) => p.light_id !== selectedLight),
                { light_id: selectedLight, x: tx, y: ty, mount },
            ]));
        }
    }
    function hitPlacement(gx, gy) {
        for (const p of placements) {
            const [mx, my] = mountOffset(p.mount);
            const dx = gx - (p.x + mx), dy = gy - (p.y + my);
            if (Math.hypot(dx, dy) < 0.3)
                return p;
        }
        return null;
    }
    function handlePointerDown(e) {
        e.target.setPointerCapture(e.pointerId);
        const panning = e.button === 1 || e.button === 2 || tool === "view";
        drag.current = { px: e.clientX, py: e.clientY, moved: false, panning };
        if (!panning) {
            const { gx, gy } = toGrid(e);
            applyTool(gx, gy, e);
        }
    }
    function handlePointerMove(e) {
        if (!drag.current)
            return;
        const dx = e.clientX - drag.current.px;
        const dy = e.clientY - drag.current.py;
        if (Math.hypot(dx, dy) > 4)
            drag.current.moved = true;
        if (drag.current.panning && drag.current.moved) {
            setView((v) => ({ ...v, ox: v.ox + dx, oy: v.oy + dy }));
            drag.current.px = e.clientX;
            drag.current.py = e.clientY;
        }
        else if (!drag.current.panning && (tool === "floor" || tool === "wall" || tool === "erase")) {
            const { gx, gy } = toGrid(e);
            applyTool(gx, gy, e);
        }
    }
    function handlePointerUp(e) {
        const wasClick = drag.current && !drag.current.moved;
        const wasPanning = drag.current?.panning;
        drag.current = null;
        if (wasClick && wasPanning && tool === "view" && e.button === 0) {
            const { gx, gy } = toGrid(e);
            const hit = hitPlacement(gx, gy);
            if (hit) {
                const k = `${hit.x},${hit.y},${hit.mount}`;
                const cluster = placements.filter((p) => `${p.x},${p.y},${p.mount}` === k);
                onLightClick(cluster, e.clientX, e.clientY);
            }
        }
    }
    function handleWheel(e) {
        const rect = canvasRef.current.getBoundingClientRect();
        const mx = e.clientX - rect.left;
        const my = e.clientY - rect.top;
        setView((v) => {
            const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
            const cell = Math.max(4, Math.min(80, v.cell * factor));
            const scale = cell / v.cell;
            return { cell, ox: mx - (mx - v.ox) * scale, oy: my - (my - v.oy) * scale };
        });
    }
    return (
    // minWidth: 0 stops the canvas's intrinsic width (set for DPR sharpness)
    // from widening this flex item — without it the layout grows every frame.
    _jsx("div", { style: { flex: 1, minWidth: 0 }, children: _jsx("canvas", { ref: canvasRef, onPointerDown: handlePointerDown, onPointerMove: handlePointerMove, onPointerUp: handlePointerUp, onWheel: handleWheel, onContextMenu: (e) => e.preventDefault(), style: {
                width: "100%",
                height: "calc(100vh - 250px)",
                minHeight: 360,
                borderRadius: 10,
                border: "1px solid #262626",
                touchAction: "none",
                cursor: tool === "view" ? "grab" : "crosshair",
                display: "block",
            } }) }));
}
