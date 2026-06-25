//! Colour palettes — named, light-agnostic colour sets imported from a
//! provider's stored scenes (today: Hue scenes via `LightProvider::discover_palettes`).
//!
//! A palette is **not** a scene: a Bifrost scene is a per-light `LightState`
//! snapshot bound to specific lights, whereas a palette is just an ordered list
//! of colours. That makes it reusable beyond its origin — applying a palette to a
//! room **distributes** its colours across whatever lights that room has (each
//! light `i` takes `colours[i % n]`), so a Hue "Tropical" scene authored for the
//! living room can be reused in any room.

use serde::{Deserialize, Serialize};

/// One entry in a palette: a colour expressed as CIE xy **or** a white
/// temperature, with an optional brightness. Mirrors a light's mutually-exclusive
/// colour-vs-temperature modes; `xy` wins when both are present.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PaletteColor {
    /// CIE xy chromaticity. `None` when this entry is a white temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xy: Option<[f32; 2]>,
    /// White point in mirek (153–500). `None` when this entry is a colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirek: Option<u16>,
    /// Brightness 0–100. `None` = leave the target light's brightness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
}

impl PaletteColor {
    /// Map this palette entry to the `LightState` a distribution writes to one
    /// light: powered on, colour **or** temperature (never both), plus brightness.
    pub fn to_light_state(&self) -> crate::models::LightState {
        let mut state = crate::models::LightState {
            on: true,
            brightness: self.brightness,
            ..Default::default()
        };
        if let Some([x, y]) = self.xy {
            state.color = Some(crate::models::Color {
                x,
                y,
                brightness: self.brightness.map(|b| b / 100.0).unwrap_or(1.0),
            });
        } else if let Some(mirek) = self.mirek {
            state.color_temp_mirek = Some(mirek);
        }
        state
    }
}

/// A stored palette.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub id: String,
    pub name: String,
    /// Provider source, e.g. `"hue"`.
    pub source: String,
    pub colors: Vec<PaletteColor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_entry_maps_to_colour_state() {
        let c = PaletteColor {
            xy: Some([0.5, 0.4]),
            mirek: None,
            brightness: Some(80.0),
        };
        let s = c.to_light_state();
        assert!(s.on);
        assert_eq!(s.brightness, Some(80.0));
        let col = s.color.expect("colour set");
        assert_eq!((col.x, col.y), (0.5, 0.4));
        assert!(
            (col.brightness - 0.8).abs() < 1e-6,
            "xyY brightness scaled 0–1"
        );
        assert!(s.color_temp_mirek.is_none(), "colour excludes temperature");
    }

    #[test]
    fn temperature_entry_maps_to_temperature_state() {
        let c = PaletteColor {
            xy: None,
            mirek: Some(300),
            brightness: None,
        };
        let s = c.to_light_state();
        assert_eq!(s.color_temp_mirek, Some(300));
        assert!(s.color.is_none());
    }

    #[test]
    fn xy_wins_when_both_present() {
        let c = PaletteColor {
            xy: Some([0.3, 0.3]),
            mirek: Some(300),
            brightness: None,
        };
        let s = c.to_light_state();
        assert!(s.color.is_some());
        assert!(s.color_temp_mirek.is_none());
    }
}
