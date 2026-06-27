//! Generic "passthrough" device domain — the long tail of source device types
//! Bifrost doesn't natively model (climate, cover/blinds, lock, vacuum, `number`,
//! `select`, `button`, …). Instead of a typed model per domain, a generic device
//! is just a set of control **primitives**, so one mapping + one fuzzy UI covers
//! dozens of Home Assistant domains at once.
//!
//! The set is deliberately tiny — almost every entity reduces to a toggle, a
//! numeric slider, an enum/select, a momentary button, or a read-only readout.
//! [`controls_from_ha`] derives those from an HA entity's domain + state +
//! attributes; the frontend renders each primitive with a generic widget.
//!
//! This is the *escape hatch* for the long tail — common, high-value device types
//! (climate especially) still graduate to a real native domain over time.

use serde::Serialize;
use serde_json::{Value, json};

/// A generic "passthrough" device — the source device plus its control
/// primitives. `provider_id` is filled by the API layer; a provider's `discover`
/// leaves it empty (it doesn't know its own DB row).
#[derive(Debug, Clone, Serialize)]
pub struct GenericDevice {
    pub provider_id: String,
    /// Provider-native id (an HA entity id, e.g. `climate.bedroom`).
    pub device_id: String,
    pub name: String,
    /// The source's domain (`climate`, `cover`, …) — drives the UI glyph/label.
    pub kind: String,
    pub controls: Vec<Control>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hw_id: Option<String>,
}

/// HA entity-id domains the generic passthrough does **not** surface — so that
/// the generic domain stays a true escape hatch: *everything else* (the
/// controllable long tail) appears automatically, and a new HA device type
/// (vacuum, valve, humidifier, …) shows up without a code change.
///
/// Two exclusion categories: (1) domains Bifrost models **natively** — surfacing
/// them generically would duplicate the device and fight cross-provider de-dup;
/// (2) **read-only / infrastructure** entities (the `sensor` flood, presence,
/// HA-internal plumbing) that aren't controllable devices. An entity that slips
/// through with no specific mapping still renders a state readout, never blank.
pub const GENERIC_HA_EXCLUDED_DOMAINS: &[&str] = &[
    // Native Bifrost domains (handled by their own providers).
    "light.",
    "switch.",
    "fan.",
    "media_player.",
    "remote.",
    // Read-only sensors — the bulk of HA's entity count.
    "sensor.",
    "binary_sensor.",
    // Presence / location / environment infra (not controllable devices).
    "device_tracker.",
    "person.",
    "zone.",
    "sun.",
    "geo_location.",
    // HA-internal plumbing & voice pipeline.
    "persistent_notification.",
    "automation.",
    "script.",
    "tag.",
    "stt.",
    "tts.",
    "conversation.",
    "assist_satellite.",
    "wake_word.",
    // Streams / one-shot data with no simple control primitive.
    "camera.",
    "image.",
    "event.",
    "update.",
];

/// Map a generic control write (entity `domain` + control `key` + JSON `value`)
/// to the Home Assistant service to call: `(service_domain, service, extra_data)`.
/// `None` = no mapping (rejected as a bad command); a wrong value type also
/// short-circuits to `None` via `?`.
pub fn control_write_to_ha(
    domain: &str,
    key: &str,
    value: &Value,
) -> Option<(String, String, Value)> {
    let svc = |d: &str, s: &str, extra: Value| Some((d.to_string(), s.to_string(), extra));
    match (domain, key) {
        ("climate", "temperature") => svc(
            "climate",
            "set_temperature",
            json!({ "temperature": value.as_f64()? }),
        ),
        ("climate", "hvac_mode") => svc(
            "climate",
            "set_hvac_mode",
            json!({ "hvac_mode": value.as_str()? }),
        ),
        ("cover", "position") => svc(
            "cover",
            "set_cover_position",
            json!({ "position": value.as_f64()? }),
        ),
        ("cover", "open") => svc("cover", "open_cover", json!({})),
        ("cover", "close") => svc("cover", "close_cover", json!({})),
        ("cover", "stop") => svc("cover", "stop_cover", json!({})),
        ("lock", "locked") => svc(
            "lock",
            if value.as_bool()? { "lock" } else { "unlock" },
            json!({}),
        ),
        ("number" | "input_number", "value") => {
            svc(domain, "set_value", json!({ "value": value.as_f64()? }))
        }
        ("select" | "input_select", "option") => svc(
            domain,
            "select_option",
            json!({ "option": value.as_str()? }),
        ),
        ("button" | "input_button", "press") => svc(domain, "press", json!({})),
        ("scene", "press") => svc("scene", "turn_on", json!({})),
        // Vacuum / robot (e.g. a Litter-Robot, which HA models as a `vacuum`):
        // momentary actions map straight to the vacuum services.
        ("vacuum", "start") => svc("vacuum", "start", json!({})),
        ("vacuum", "pause") => svc("vacuum", "pause", json!({})),
        ("vacuum", "stop") => svc("vacuum", "stop", json!({})),
        ("vacuum", "return") => svc("vacuum", "return_to_base", json!({})),
        ("vacuum", "clean_spot") => svc("vacuum", "clean_spot", json!({})),
        ("vacuum", "locate") => svc("vacuum", "locate", json!({})),
        ("vacuum", "fan_speed") => svc(
            "vacuum",
            "set_fan_speed",
            json!({ "fan_speed": value.as_str()? }),
        ),
        _ => None,
    }
}

/// One control primitive on a generic device. `key` identifies it for writes
/// (it maps back to a provider service/field).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Control {
    /// On/off (switch, lock, fan, boolean helper).
    Toggle {
        key: String,
        label: String,
        value: bool,
    },
    /// A numeric value with bounds (target temp, cover position, `number`, fan %).
    Number {
        key: String,
        label: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// A choice from a fixed set (hvac mode, fan preset, `select`).
    Enum {
        key: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        options: Vec<String>,
    },
    /// A momentary action (`button`, `scene`, cover open/close/stop).
    Button { key: String, label: String },
    /// Read-only value (current temperature, battery, any `sensor`).
    Readout {
        key: String,
        label: String,
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
}

fn attr_f64(attrs: &Value, k: &str) -> Option<f64> {
    attrs.get(k).and_then(Value::as_f64)
}
fn attr_str(attrs: &Value, k: &str) -> Option<String> {
    attrs.get(k).and_then(Value::as_str).map(String::from)
}
fn attr_list(attrs: &Value, k: &str) -> Vec<String> {
    attrs
        .get(k)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Derive the control primitives for one Home Assistant entity from its `domain`
/// (the bit before the dot in the entity id), string `state`, and `attributes`.
/// Unmapped domains fall back to a single read-only state readout, so nothing is
/// ever blank.
pub fn controls_from_ha(domain: &str, state: &str, attrs: &Value) -> Vec<Control> {
    match domain {
        "climate" => {
            let mut c = Vec::new();
            if let Some(t) = attr_f64(attrs, "temperature") {
                c.push(Control::Number {
                    key: "temperature".into(),
                    label: "Target".into(),
                    value: t,
                    min: attr_f64(attrs, "min_temp").unwrap_or(7.0),
                    max: attr_f64(attrs, "max_temp").unwrap_or(35.0),
                    step: attr_f64(attrs, "target_temp_step").unwrap_or(0.5),
                    unit: attr_str(attrs, "temperature_unit").or_else(|| Some("°".into())),
                });
            }
            let modes = attr_list(attrs, "hvac_modes");
            if !modes.is_empty() {
                c.push(Control::Enum {
                    key: "hvac_mode".into(),
                    label: "Mode".into(),
                    value: Some(state.to_string()),
                    options: modes,
                });
            }
            if let Some(ct) = attr_f64(attrs, "current_temperature") {
                c.push(Control::Readout {
                    key: "current_temperature".into(),
                    label: "Current".into(),
                    value: format_num(ct),
                    unit: attr_str(attrs, "temperature_unit"),
                });
            }
            c
        }
        "cover" => {
            let mut c = Vec::new();
            if let Some(p) = attr_f64(attrs, "current_position") {
                c.push(Control::Number {
                    key: "position".into(),
                    label: "Position".into(),
                    value: p,
                    min: 0.0,
                    max: 100.0,
                    step: 1.0,
                    unit: Some("%".into()),
                });
            }
            c.push(Control::Button {
                key: "open".into(),
                label: "Open".into(),
            });
            c.push(Control::Button {
                key: "close".into(),
                label: "Close".into(),
            });
            c.push(Control::Button {
                key: "stop".into(),
                label: "Stop".into(),
            });
            c
        }
        "lock" => vec![Control::Toggle {
            key: "locked".into(),
            label: "Locked".into(),
            value: state == "locked",
        }],
        "number" | "input_number" => vec![Control::Number {
            key: "value".into(),
            label: attr_str(attrs, "friendly_name").unwrap_or_else(|| "Value".into()),
            value: state.parse().unwrap_or(0.0),
            min: attr_f64(attrs, "min").unwrap_or(0.0),
            max: attr_f64(attrs, "max").unwrap_or(100.0),
            step: attr_f64(attrs, "step").unwrap_or(1.0),
            unit: attr_str(attrs, "unit_of_measurement"),
        }],
        "select" | "input_select" => vec![Control::Enum {
            key: "option".into(),
            label: "Option".into(),
            value: Some(state.to_string()),
            options: attr_list(attrs, "options"),
        }],
        "button" | "input_button" | "scene" => {
            vec![Control::Button {
                key: "press".into(),
                label: "Press".into(),
            }]
        }
        "weather" => {
            // A weather entity's `state` is the condition (e.g. "partlycloudy");
            // temperature/humidity are attributes. Surfaced as readouts — the Boards
            // weather widget reads `condition` + `temperature` and draws the icon.
            let mut c = vec![Control::Readout {
                key: "condition".into(),
                label: "Condition".into(),
                value: state.to_string(),
                unit: None,
            }];
            if let Some(t) = attr_f64(attrs, "temperature") {
                c.push(Control::Readout {
                    key: "temperature".into(),
                    label: "Temperature".into(),
                    value: format_num(t),
                    unit: attr_str(attrs, "temperature_unit").or_else(|| Some("°".into())),
                });
            }
            if let Some(h) = attr_f64(attrs, "humidity") {
                c.push(Control::Readout {
                    key: "humidity".into(),
                    label: "Humidity".into(),
                    value: format_num(h),
                    unit: Some("%".into()),
                });
            }
            c
        }
        "vacuum" => {
            // A vacuum/robot (e.g. a Litter-Robot): status readout + the actions
            // its `supported_features` bitmask advertises, plus battery/mode when
            // reported. Bits are `VacuumEntityFeature`.
            let mut c = vec![Control::Readout {
                key: "state".into(),
                label: "Status".into(),
                value: state.to_string(),
                unit: None,
            }];
            let feat = attr_f64(attrs, "supported_features").unwrap_or(0.0) as u64;
            for (bit, key, label) in [
                (8192u64, "start", "Start"),
                (4, "pause", "Pause"),
                (8, "stop", "Stop"),
                (16, "return", "Return home"),
                (1024, "clean_spot", "Clean spot"),
                (512, "locate", "Locate"),
            ] {
                if feat & bit != 0 {
                    c.push(Control::Button {
                        key: key.into(),
                        label: label.into(),
                    });
                }
            }
            if let Some(b) = attr_f64(attrs, "battery_level") {
                c.push(Control::Readout {
                    key: "battery".into(),
                    label: "Battery".into(),
                    value: format_num(b),
                    unit: Some("%".into()),
                });
            }
            let speeds = attr_list(attrs, "fan_speed_list");
            if !speeds.is_empty() {
                c.push(Control::Enum {
                    key: "fan_speed".into(),
                    label: "Mode".into(),
                    value: attr_str(attrs, "fan_speed"),
                    options: speeds,
                });
            }
            c
        }
        // Unknown domain: surface the raw state as a readout so it's never blank.
        _ => vec![Control::Readout {
            key: "state".into(),
            label: "State".into(),
            value: state.to_string(),
            unit: attr_str(attrs, "unit_of_measurement"),
        }],
    }
}

/// Trim a trailing `.0` so a whole number reads "21" not "21.0".
fn format_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn climate_yields_target_mode_and_current() {
        let attrs = json!({
            "temperature": 21.0, "min_temp": 7, "max_temp": 35, "target_temp_step": 0.5,
            "current_temperature": 19.5, "hvac_modes": ["off", "heat", "cool"],
            "temperature_unit": "°C"
        });
        let c = controls_from_ha("climate", "heat", &attrs);
        assert!(
            matches!(&c[0], Control::Number { key, value, max, .. } if key == "temperature" && *value == 21.0 && *max == 35.0)
        );
        assert!(matches!(&c[1], Control::Enum { key, value, options, .. }
            if key == "hvac_mode" && value.as_deref() == Some("heat") && options.len() == 3));
        assert!(
            matches!(&c[2], Control::Readout { key, value, .. } if key == "current_temperature" && value == "19.5")
        );
    }

    #[test]
    fn cover_yields_position_and_buttons() {
        let c = controls_from_ha("cover", "open", &json!({ "current_position": 60 }));
        assert!(
            matches!(&c[0], Control::Number { key, value, .. } if key == "position" && *value == 60.0)
        );
        assert_eq!(
            c.iter()
                .filter(|x| matches!(x, Control::Button { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn lock_select_number_button_map() {
        assert!(matches!(
            &controls_from_ha("lock", "locked", &json!({}))[0],
            Control::Toggle { value: true, .. }
        ));
        assert!(
            matches!(&controls_from_ha("select", "Eco", &json!({ "options": ["Eco", "Boost"] }))[0],
            Control::Enum { value, options, .. } if value.as_deref() == Some("Eco") && options.len() == 2)
        );
        assert!(
            matches!(&controls_from_ha("number", "42", &json!({ "min": 0, "max": 100, "step": 2, "unit_of_measurement": "%" }))[0],
            Control::Number { value, step, .. } if *value == 42.0 && *step == 2.0)
        );
        assert!(matches!(
            &controls_from_ha("button", "unknown", &json!({}))[0],
            Control::Button { .. }
        ));
    }

    #[test]
    fn unknown_domain_falls_back_to_a_readout() {
        let c = controls_from_ha("water_heater", "eco", &json!({}));
        assert!(matches!(&c[0], Control::Readout { value, .. } if value == "eco"));
    }

    #[test]
    fn weather_yields_condition_temperature_and_humidity() {
        let c = controls_from_ha(
            "weather",
            "partlycloudy",
            &json!({ "temperature": 18.5, "temperature_unit": "°C", "humidity": 64 }),
        );
        assert!(
            matches!(&c[0], Control::Readout { key, value, .. } if key == "condition" && value == "partlycloudy")
        );
        assert!(
            c.iter()
                .any(|x| matches!(x, Control::Readout { key, value, unit, .. }
                if key == "temperature" && value == "18.5" && unit.as_deref() == Some("°C")))
        );
        assert!(
            c.iter()
                .any(|x| matches!(x, Control::Readout { key, unit, .. }
                if key == "humidity" && unit.as_deref() == Some("%")))
        );
    }

    #[test]
    fn vacuum_yields_status_and_feature_gated_buttons() {
        // 12296 = START(8192) | STATE(4096) | STOP(8) — the Litter-Robot 4's set.
        let c = controls_from_ha("vacuum", "docked", &json!({ "supported_features": 12296 }));
        assert!(
            matches!(&c[0], Control::Readout { key, value, .. } if key == "state" && value == "docked")
        );
        let buttons: Vec<&str> = c
            .iter()
            .filter_map(|x| match x {
                Control::Button { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect();
        assert!(buttons.contains(&"start") && buttons.contains(&"stop"));
        // Bits not set → those actions are absent.
        assert!(!buttons.contains(&"pause") && !buttons.contains(&"return"));
    }

    #[test]
    fn vacuum_write_map_targets_vacuum_services() {
        assert_eq!(
            control_write_to_ha("vacuum", "start", &json!(null))
                .unwrap()
                .1,
            "start"
        );
        assert_eq!(
            control_write_to_ha("vacuum", "return", &json!(null))
                .unwrap()
                .1,
            "return_to_base"
        );
        let (_, s, data) = control_write_to_ha("vacuum", "fan_speed", &json!("max")).unwrap();
        assert_eq!(s, "set_fan_speed");
        assert_eq!(data["fan_speed"], "max");
    }

    #[test]
    fn denylist_excludes_native_and_sensors_but_not_the_controllable_longtail() {
        // Native domains + read-only noise are excluded …
        for excluded in [
            "light.",
            "switch.",
            "media_player.",
            "sensor.",
            "binary_sensor.",
        ] {
            assert!(
                GENERIC_HA_EXCLUDED_DOMAINS.contains(&excluded),
                "{excluded} should be excluded"
            );
        }
        // … but the controllable long tail (incl. types with no specific mapping
        // yet) is NOT — that's the escape-hatch guarantee.
        for surfaced in ["vacuum.", "valve.", "humidifier.", "climate.", "lock."] {
            assert!(
                !GENERIC_HA_EXCLUDED_DOMAINS.contains(&surfaced),
                "{surfaced} should be surfaced generically"
            );
        }
    }

    #[test]
    fn write_map_targets_the_right_ha_service() {
        let (d, s, data) = control_write_to_ha("climate", "temperature", &json!(21.0)).unwrap();
        assert_eq!((d.as_str(), s.as_str()), ("climate", "set_temperature"));
        assert_eq!(data["temperature"], 21.0);
        assert_eq!(
            control_write_to_ha("cover", "open", &json!(null))
                .unwrap()
                .1,
            "open_cover"
        );
        assert_eq!(
            control_write_to_ha("lock", "locked", &json!(false))
                .unwrap()
                .1,
            "unlock"
        );
        let (d, s, data) = control_write_to_ha("select", "option", &json!("Eco")).unwrap();
        assert_eq!((d.as_str(), s.as_str()), ("select", "select_option"));
        assert_eq!(data["option"], "Eco");
        // Bad value type and unknown mapping both reject.
        assert!(control_write_to_ha("climate", "temperature", &json!("hot")).is_none());
        assert!(control_write_to_ha("vacuum", "spin", &json!(null)).is_none());
    }
}
