//! Sensor-device domain: read-only environmental / presence inputs — motion,
//! occupancy, contact (door/window), illuminance (lux), temperature, humidity,
//! … — that a provider reports but that nothing writes to.
//!
//! This is the leanest domain: a sensor has **no controllable surface at all**,
//! so unlike lights/media/power there is no command type — only a [`SensorState`]
//! (a reading plus reachability). What differs between sensors is the
//! [`SensorKind`] (which drives the glyph and, crucially, whether the sensor
//! counts as **presence**) and the reading's unit.
//!
//! The shape otherwise mirrors [`crate::models::power::PowerDevice`]: discovery
//! returns full device snapshots and reads return full state. Rooms aggregate the
//! presence kinds into `Room.occupied`, so presence-driven behaviour is
//! provider-agnostic (a Hue motion sensor and an HA `binary_sensor` are
//! interchangeable inputs).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A read-only sensor. `kind` selects the glyph and semantics; `unit` is the
/// reading's display unit when the provider reports one (°C, lx, %).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorDevice {
    pub id: Uuid,
    /// Stable provider-specific identifier (e.g. an HA `entity_id`, a Hue
    /// resource id).
    pub provider_id: String,
    pub name: String,
    pub kind: SensorKind,
    pub state: SensorState,
    /// Display unit for a numeric reading (°C, lx, %); `None` for booleans or
    /// when the provider doesn't report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Normalized hardware identity for cross-provider de-dup (see
    /// [`crate::providers::mac_hw_id`]); `None` when the provider can't supply one.
    #[serde(default)]
    pub hw_id: Option<String>,
}

/// The flavour of sensor — chosen so the UI can pick a glyph and so Rooms know
/// which sensors contribute to occupancy. Keep the list small and glyphable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// A motion detector (Hue SML, HA `binary_sensor` device_class `motion`).
    Motion,
    /// A presence / occupancy sensor (mmWave, HA `occupancy`/`presence`).
    Occupancy,
    /// A door/window contact (open/closed).
    Contact,
    /// Ambient light level, in lux.
    Illuminance,
    /// Temperature (unit carried on the device — usually °C).
    Temperature,
    /// Relative humidity, in percent.
    Humidity,
    /// Unknown or unclassified read-only value.
    #[default]
    Generic,
}

impl SensorKind {
    /// Whether this kind contributes to room occupancy — i.e. a `true` boolean
    /// reading means "someone is here". Motion and occupancy do; a contact sensor
    /// (a door left open) or a numeric reading do not.
    pub fn is_presence(self) -> bool {
        matches!(self, SensorKind::Motion | SensorKind::Occupancy)
    }
}

/// One sensor reading — either a boolean (motion detected, contact open) or a
/// number (lux, temperature, humidity). Externally tagged so the JSON is
/// self-describing: `{"bool":true}` / `{"number":21.5}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorReading {
    Bool(bool),
    Number(f64),
}

impl SensorReading {
    /// The boolean value, if this is a boolean reading.
    pub fn as_bool(self) -> Option<bool> {
        match self {
            SensorReading::Bool(b) => Some(b),
            SensorReading::Number(_) => None,
        }
    }

    /// The numeric value, if this is a numeric reading.
    pub fn as_number(self) -> Option<f64> {
        match self {
            SensorReading::Number(n) => Some(n),
            SensorReading::Bool(_) => None,
        }
    }
}

/// Full sensor state: the latest reading plus reachability. A sensor that has
/// never reported (or is unreachable) carries `reading: None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SensorState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<SensorReading>,
    /// Whether the device is reachable by its provider (`None` = the provider
    /// doesn't report it). Mirrors `PowerState::reachable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
    /// When the reading last **changed**, as the provider reports it (RFC 3339
    /// UTC — Hue's `motion_report.changed`, HA's `last_changed`). `None` when
    /// the provider doesn't say. Display-only: the engine detects edges from
    /// the readings themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<String>,
}

impl SensorState {
    /// A convenience constructor for a boolean reading (motion/contact).
    pub fn boolean(on: bool) -> Self {
        Self {
            reading: Some(SensorReading::Bool(on)),
            reachable: Some(true),
            changed_at: None,
        }
    }

    /// A convenience constructor for a numeric reading (lux/temp/humidity).
    pub fn number(value: f64) -> Self {
        Self {
            reading: Some(SensorReading::Number(value)),
            reachable: Some(true),
            changed_at: None,
        }
    }

    /// Whether this reading is a boolean `true` — used by presence aggregation.
    pub fn is_detecting(&self) -> bool {
        matches!(self.reading, Some(SensorReading::Bool(true)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_kind_serialises_snake_case() {
        assert_eq!(
            serde_json::to_string(&SensorKind::Illuminance).unwrap(),
            "\"illuminance\""
        );
        assert_eq!(
            serde_json::to_string(&SensorKind::Occupancy).unwrap(),
            "\"occupancy\""
        );
    }

    #[test]
    fn only_motion_and_occupancy_are_presence() {
        assert!(SensorKind::Motion.is_presence());
        assert!(SensorKind::Occupancy.is_presence());
        for k in [
            SensorKind::Contact,
            SensorKind::Illuminance,
            SensorKind::Temperature,
            SensorKind::Humidity,
            SensorKind::Generic,
        ] {
            assert!(!k.is_presence(), "{k:?} must not count as presence");
        }
    }

    #[test]
    fn sensor_kind_defaults_to_generic() {
        assert_eq!(SensorKind::default(), SensorKind::Generic);
    }

    #[test]
    fn reading_accessors_are_type_specific() {
        assert_eq!(SensorReading::Bool(true).as_bool(), Some(true));
        assert_eq!(SensorReading::Bool(true).as_number(), None);
        assert_eq!(SensorReading::Number(21.5).as_number(), Some(21.5));
        assert_eq!(SensorReading::Number(21.5).as_bool(), None);
    }

    #[test]
    fn reading_json_is_externally_tagged() {
        assert_eq!(
            serde_json::to_string(&SensorReading::Bool(true)).unwrap(),
            r#"{"bool":true}"#
        );
        assert_eq!(
            serde_json::to_string(&SensorReading::Number(21.5)).unwrap(),
            r#"{"number":21.5}"#
        );
    }

    #[test]
    fn is_detecting_only_for_boolean_true() {
        assert!(SensorState::boolean(true).is_detecting());
        assert!(!SensorState::boolean(false).is_detecting());
        assert!(!SensorState::number(500.0).is_detecting());
        assert!(!SensorState::default().is_detecting());
    }

    #[test]
    fn empty_state_omits_optional_fields() {
        assert_eq!(
            serde_json::to_string(&SensorState::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn sensor_device_roundtrips() {
        let d = SensorDevice {
            id: Uuid::new_v4(),
            provider_id: "binary_sensor.hall_motion".into(),
            name: "Hall motion".into(),
            kind: SensorKind::Motion,
            state: SensorState::boolean(true),
            unit: None,
            hw_id: Some("mac:001788abcdef".into()),
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: SensorDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider_id, "binary_sensor.hall_motion");
        assert_eq!(back.kind, SensorKind::Motion);
        assert!(back.state.is_detecting());
        assert_eq!(back.hw_id.as_deref(), Some("mac:001788abcdef"));
    }
}
