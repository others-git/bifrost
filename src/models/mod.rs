pub mod audio;
pub mod power;
pub mod remote;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Normalized light state shared across all providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Light {
    pub id: Uuid,
    /// Stable provider-specific identifier (e.g. Hue resource UUID, Govee device id).
    pub provider_id: String,
    pub provider: Provider,
    pub name: String,
    pub state: LightState,
    pub capabilities: LightCapabilities,
    pub last_seen: DateTime<Utc>,
    /// Normalized hardware identity for cross-provider de-dup (see
    /// [`crate::providers::mac_hw_id`]); `None` when the provider can't supply one.
    #[serde(default)]
    pub hw_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Hue,
    Govee,
    Lifx,
    Wled,
    Tasmota,
    Shelly,
    /// Home Assistant — a "high-class" adapter that surfaces any of HA's
    /// integrations (and its Areas as Bifrost Rooms) through one provider.
    Ha,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LightState {
    pub on: bool,
    /// 0–100
    pub brightness: Option<f32>,
    pub color: Option<Color>,
    /// Color temperature in mirek (153–500 ≈ 6500K–2000K).
    pub color_temp_mirek: Option<u16>,
    /// Whether the device is reachable by its provider (None = the provider
    /// doesn't report it). An unreachable light also reports `on: false` —
    /// cloud APIs return stale power state for offline devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
    /// Active dynamic effect (a name from [`LightCapabilities::effects`], e.g.
    /// "candle"), or `"no_effect"` when none is running. None = the provider
    /// doesn't report effects / a partial update doesn't touch it. Setting it on
    /// a `set_state` applies the effect (the device animates color/brightness).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    /// How the device is currently being reached, for providers that have more
    /// than one transport (today: Govee — `"lan"` when controlled over the local
    /// network, `"cloud"` when via the cloud API). `None` = single-transport
    /// provider / not reported. Read-only status (like [`reachable`]); never sent
    /// on a control write — the frontend surfaces it on the Devices page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
}

/// A partial light-state update. Hue SSE events only carry the fields that
/// changed; treating them as full states (with defaults for the rest) was
/// corrupting stored state — e.g. a brightness-only event implied `on: false`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LightStatePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_temp_mirek: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
}

impl LightStatePatch {
    /// A patch carrying every field of a full state (used by pollers, whose
    /// reads are authoritative).
    pub fn from_full(s: &LightState) -> Self {
        Self {
            on: Some(s.on),
            brightness: s.brightness,
            color: s.color.clone(),
            color_temp_mirek: s.color_temp_mirek,
            reachable: s.reachable,
            effect: s.effect.clone(),
            transport: s.transport.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.on.is_none()
            && self.brightness.is_none()
            && self.color.is_none()
            && self.color_temp_mirek.is_none()
            && self.reachable.is_none()
            && self.effect.is_none()
            && self.transport.is_none()
    }

    /// Merge this patch into an existing state, leaving absent fields untouched.
    pub fn apply_to(&self, state: &mut LightState) {
        if let Some(on) = self.on {
            state.on = on;
        }
        if let Some(b) = self.brightness {
            state.brightness = Some(b);
        }
        if let Some(r) = self.reachable {
            state.reachable = Some(r);
        }
        if let Some(t) = &self.transport {
            state.transport = Some(t.clone());
        }
        // Colour, colour-temperature, and a dynamic effect are mutually exclusive
        // modes — a light is in exactly one. A patch that sets any one clears the
        // other two so the merged state names a single honest mode (and a scene
        // snapshot never carries a stale effect onto a now-plain-colour light).
        if let Some(c) = &self.color {
            state.color = Some(c.clone());
            state.color_temp_mirek = None;
            state.effect = None;
        } else if let Some(m) = self.color_temp_mirek {
            state.color_temp_mirek = Some(m);
            state.color = None;
            state.effect = None;
        } else if let Some(e) = &self.effect {
            if is_clear_effect(e) {
                state.effect = None;
            } else {
                state.effect = Some(e.clone());
                state.color = None;
                state.color_temp_mirek = None;
            }
        }
    }
}

/// The provider-specific tokens that all mean "no effect is running" — Hue's
/// `no_effect`, LIFX's `off`, HA's `None`, or an empty string. Such an effect is
/// stored as a cleared (`None`) effect, keeping a light in exactly one mode.
pub fn is_clear_effect(effect: &str) -> bool {
    matches!(
        effect.trim().to_ascii_lowercase().as_str(),
        "" | "no_effect" | "off" | "none"
    )
}

/// CIE 1931 xy + brightness, as used by Hue. Govee colors are converted to/from RGB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Color {
    /// CIE xy (Hue-native). Derived from RGB for Govee devices.
    pub x: f32,
    pub y: f32,
    /// Linear brightness 0.0–1.0 (the Y component of xyY).
    pub brightness: f32,
}

impl Color {
    /// Convert sRGB (0–255 each) to CIE xy + Y brightness.
    /// Uses the wide sRGB matrix; for Hue gamut-clipping see `providers::hue::color`.
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        let r = srgb_linearize(r);
        let g = srgb_linearize(g);
        let b = srgb_linearize(b);

        // Philips Hue Wide RGB D65 matrix (matches the inverse used in to_rgb)
        let x = r * 0.664511 + g * 0.154324 + b * 0.162028;
        let y = r * 0.283881 + g * 0.668433 + b * 0.047685;
        let z = r * 0.000088 + g * 0.072310 + b * 0.986039;

        let sum = x + y + z;
        if sum == 0.0 {
            return Self {
                x: 0.0,
                y: 0.0,
                brightness: 0.0,
            };
        }
        Self {
            x: x / sum,
            y: y / sum,
            brightness: y,
        }
    }

    /// Convert CIE xy + Y back to sRGB (0–255 each), clamped.
    pub fn to_rgb(&self) -> (u8, u8, u8) {
        let y = self.brightness;
        let x = (y / self.y) * self.x;
        let z = (y / self.y) * (1.0 - self.x - self.y);

        // Inverse Hue Wide RGB D65 matrix (consistent with from_rgb)
        let r = x * 1.656492 - y * 0.354851 - z * 0.255038;
        let g = -x * 0.707196 + y * 1.655397 + z * 0.036152;
        let b = x * 0.051713 - y * 0.121364 + z * 1.011_53;

        let r = srgb_gamma(r.max(0.0));
        let g = srgb_gamma(g.max(0.0));
        let b = srgb_gamma(b.max(0.0));

        (
            (r * 255.0).round().clamp(0.0, 255.0) as u8,
            (g * 255.0).round().clamp(0.0, 255.0) as u8,
            (b * 255.0).round().clamp(0.0, 255.0) as u8,
        )
    }
}

fn srgb_linearize(channel: u8) -> f32 {
    let c = channel as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055_f32).powf(2.4)
    }
}

fn srgb_gamma(linear: f32) -> f32 {
    if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LightCapabilities {
    pub dimmable: bool,
    pub color_rgb: bool,
    pub color_temperature: bool,
    /// Hue gamut type (A, B, or C); None for non-Hue or unknown.
    pub hue_gamut: Option<HueGamut>,
    /// Dynamic effects this light supports (provider-native names, e.g.
    /// "candle"/"fire"; includes "no_effect" to clear). Empty = no effects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<String>,
    /// Number of independently addressable colour **segments** this light exposes
    /// (e.g. a Govee strip's per-section LEDs), or `None` when it has none. The UI
    /// shows a per-segment editor only when this is set. Segment colours are a
    /// write-only control (providers don't report them back), so they live outside
    /// [`LightState`] — see [`SegmentColor`] and `LightProvider::set_segments`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<u16>,
}

/// One segment's target for a per-segment write (`LightProvider::set_segments`):
/// a colour, a brightness, or both. Colour and brightness are independent Govee
/// capabilities, so each field is optional — a colour pick sets `rgb`, the
/// relative-brightness slider sets `brightness`. Write-only: providers expose
/// segment *control* but not segment *readback*, so this isn't part of [`LightState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentColor {
    /// Zero-based segment index (`0..capabilities.segments`).
    pub segment: u16,
    /// Packed 24-bit colour, `0xRRGGBB`. `None` = leave this segment's colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb: Option<u32>,
    /// Per-segment brightness 0–100. `None` = leave this segment's brightness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
}

/// Hue color gamut bounds in CIE xy.
/// See references/hue_color_gamut.png for the visual diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HueGamut {
    /// Older Hue bulbs (LivingColors gen1–2, classic bulbs).
    A,
    /// LivingColors gen3, Studio gen3.
    B,
    /// Current generation (LCA001, LCT015, etc.).
    C,
}

impl HueGamut {
    /// Triangle vertices (red, green, blue) in CIE xy.
    pub fn vertices(&self) -> [(f32, f32); 3] {
        match self {
            Self::A => [(0.7040, 0.2960), (0.2151, 0.7106), (0.1380, 0.0800)],
            Self::B => [(0.6750, 0.3220), (0.4090, 0.5180), (0.1670, 0.0400)],
            Self::C => [(0.6915, 0.3083), (0.1700, 0.7000), (0.1532, 0.0475)],
        }
    }

    /// Clamp a CIE xy point to the closest point inside this gamut triangle.
    pub fn clamp(&self, x: f32, y: f32) -> (f32, f32) {
        let [r, g, b] = self.vertices();
        closest_point_in_triangle(x, y, r, g, b)
    }
}

/// Project point P onto the closest edge of triangle (A, B, C) if P is outside.
fn closest_point_in_triangle(
    px: f32,
    py: f32,
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
) -> (f32, f32) {
    if point_in_triangle(px, py, a, b, c) {
        return (px, py);
    }
    let ca = closest_on_segment(px, py, a, b);
    let cb = closest_on_segment(px, py, b, c);
    let cc = closest_on_segment(px, py, c, a);
    [ca, cb, cc]
        .into_iter()
        .min_by(|p, q| dist2(px, py, *p).partial_cmp(&dist2(px, py, *q)).unwrap())
        .unwrap()
}

fn point_in_triangle(px: f32, py: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let sign = |p: (f32, f32), q: (f32, f32)| (px - q.0) * (p.1 - q.1) - (p.0 - q.0) * (py - q.1);
    let d1 = sign(a, b);
    let d2 = sign(b, c);
    let d3 = sign(c, a);
    let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(has_neg && has_pos)
}

fn closest_on_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return a;
    }
    let t = ((px - ax) * dx + (py - ay) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    (ax + t * dx, ay + t * dy)
}

fn dist2(px: f32, py: f32, q: (f32, f32)) -> f32 {
    (px - q.0).powi(2) + (py - q.1).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_roundtrip() {
        let c = Color::from_rgb(255, 128, 0);
        let (r, g, b) = c.to_rgb();
        assert!((r as i32 - 255).abs() <= 2);
        assert!((g as i32 - 128).abs() <= 2);
        assert!(b <= 5);
    }

    #[test]
    fn gamut_c_white_is_inside() {
        let white = Color::from_rgb(255, 255, 255);
        let (cx, cy) = HueGamut::C.clamp(white.x, white.y);
        assert!((cx - white.x).abs() < 0.01);
        assert!((cy - white.y).abs() < 0.01);
    }

    #[test]
    fn clear_effect_tokens_are_recognized() {
        for t in ["no_effect", "off", "None", "", "  OFF  "] {
            assert!(is_clear_effect(t), "{t:?} should be a clear token");
        }
        assert!(!is_clear_effect("candle"));
        assert!(!is_clear_effect("breathe"));
    }

    #[test]
    fn patch_effect_clears_color_and_temp_and_vice_versa() {
        // An effect patch puts the light into effect mode, dropping colour/temp.
        let mut s = LightState {
            on: true,
            color: Some(Color::from_rgb(255, 0, 0)),
            color_temp_mirek: None,
            ..Default::default()
        };
        LightStatePatch {
            effect: Some("candle".into()),
            ..Default::default()
        }
        .apply_to(&mut s);
        assert_eq!(s.effect.as_deref(), Some("candle"));
        assert!(s.color.is_none(), "effect mode clears colour");

        // A colour patch then drops the effect again.
        LightStatePatch {
            color: Some(Color::from_rgb(0, 255, 0)),
            ..Default::default()
        }
        .apply_to(&mut s);
        assert!(s.color.is_some());
        assert_eq!(s.effect, None, "colour mode clears the effect");

        // A clear-token effect just stops the effect without entering effect mode.
        s.color = Some(Color::from_rgb(0, 0, 255));
        LightStatePatch {
            effect: Some("no_effect".into()),
            ..Default::default()
        }
        .apply_to(&mut s);
        assert_eq!(s.effect, None);
        assert!(s.color.is_some(), "clearing an effect leaves colour intact");
    }
}
