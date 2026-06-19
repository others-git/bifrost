//! Power-device domain: the DRY home for "strictly on/off" devices — switches,
//! smart plugs/outlets, fans, boolean helpers, … — that a provider can toggle
//! but that carry no richer state than power.
//!
//! Rather than give every such device its own model with a mostly-empty
//! capability set (a fan is not "a light without color"), they share **one lean
//! model**: the only state is `on`. What differs between them is purely
//! cosmetic — a `PowerKind` that drives the UI glyph (a fan icon vs a plug vs a
//! switch). Devices with genuinely richer state (climate, covers, locks, …) get
//! their own domain instead of being crammed in here; this keeps each domain's
//! state shape honest.
//!
//! The shape mirrors `MediaDevice`/`Light`: discovery returns full device
//! snapshots and reads return full state, but a power "command" is just a bool.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A device whose entire controllable surface is on/off. The `kind` is for
/// presentation only (which glyph to show) — it does not change behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerDevice {
    pub id: Uuid,
    /// Stable provider-specific identifier (e.g. an HA `entity_id`).
    pub provider_id: String,
    pub name: String,
    pub kind: PowerKind,
    pub state: PowerState,
    /// Normalized hardware identity for cross-provider de-dup (see
    /// [`crate::providers::mac_hw_id`]); `None` when the provider can't supply one.
    #[serde(default)]
    pub hw_id: Option<String>,
}

/// The flavour of power device — chosen so the UI can pick a meaningful glyph
/// without needing per-provider knowledge. Keep this list small and glyphable;
/// anything that needs *more than a glyph's worth* of distinction probably wants
/// its own domain, not a new variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PowerKind {
    /// A wall/relay switch controlling some load.
    Switch,
    /// A smart plug / socket / outlet.
    Outlet,
    /// A fan (on/off only — speed/oscillation would be a richer domain).
    Fan,
    /// A virtual toggle / boolean helper (e.g. HA `input_boolean`).
    Toggle,
    /// Unknown or unclassified on/off device.
    #[default]
    Generic,
}

/// Full power-device state snapshot. Deliberately minimal — that minimalism is
/// the point of the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PowerState {
    pub on: bool,
    /// Whether the device is reachable by its provider (`None` = the provider
    /// doesn't report it). Mirrors `LightState::reachable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_kind_serialises_snake_case() {
        assert_eq!(
            serde_json::to_string(&PowerKind::Outlet).unwrap(),
            "\"outlet\""
        );
        assert_eq!(
            serde_json::to_string(&PowerKind::Toggle).unwrap(),
            "\"toggle\""
        );
    }

    #[test]
    fn power_kind_defaults_to_generic() {
        assert_eq!(PowerKind::default(), PowerKind::Generic);
    }

    #[test]
    fn power_state_omits_reachable_when_absent() {
        // A lean default state serialises without a null `reachable` field.
        let json = serde_json::to_string(&PowerState {
            on: true,
            reachable: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"on":true}"#);
    }

    #[test]
    fn power_device_roundtrips() {
        let d = PowerDevice {
            id: Uuid::new_v4(),
            provider_id: "switch.kitchen".into(),
            name: "Kitchen".into(),
            kind: PowerKind::Switch,
            state: PowerState {
                on: false,
                reachable: Some(true),
            },
            hw_id: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: PowerDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider_id, "switch.kitchen");
        assert_eq!(back.kind, PowerKind::Switch);
        assert!(!back.state.on);
        assert_eq!(back.state.reachable, Some(true));
    }
}
