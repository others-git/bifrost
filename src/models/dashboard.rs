//! User-composable dashboards ("Boards") — a named board is a free-form grid of
//! widgets the user drags, resizes, and configures. The backend is a **generic
//! layout store**: it persists each widget's grid box and an opaque `config`
//! verbatim, and the **frontend owns every widget type's semantics**, so new
//! widget types (a control button, a now-playing tile, …) need no backend change.

use serde::{Deserialize, Serialize};

/// One placed widget on a board. `kind` (`device` | `room` | `control` | `scene` |
/// `now_playing` | …) and `config` are frontend-defined and stored verbatim;
/// `x`/`y` are grid-cell coordinates and `w`/`h` are cell spans.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Widget {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Type-specific configuration, opaque to the backend (e.g. a device id, a room
    /// id, or a reused room-control spec for a custom control button).
    #[serde(default)]
    pub config: serde_json::Value,
}

/// A user-composed dashboard ("Board"): a name, an order position, and its widgets.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub widgets: Vec<Widget>,
}

/// Parse a stored `layout` JSON column into widgets, tolerating a malformed/empty
/// value (→ no widgets) so a board never fails to load.
pub fn parse_layout(layout: &str) -> Vec<Widget> {
    serde_json::from_str(layout).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_roundtrips_with_type_rename_and_config() {
        let w = Widget {
            id: "w1".into(),
            kind: "device".into(),
            x: 2,
            y: 0,
            w: 3,
            h: 2,
            config: serde_json::json!({ "device_id": "abc", "domain": "light" }),
        };
        let json = serde_json::to_string(&w).unwrap();
        // `type` is the wire key (not `kind`).
        assert!(json.contains("\"type\":\"device\""));
        assert_eq!(parse_layout(&format!("[{json}]")), vec![w]);
    }

    #[test]
    fn parse_layout_tolerates_garbage_and_empty() {
        assert!(parse_layout("").is_empty());
        assert!(parse_layout("not json").is_empty());
        assert!(parse_layout("[]").is_empty());
    }

    #[test]
    fn widget_config_defaults_to_null_when_absent() {
        let w = &parse_layout(r#"[{"id":"a","type":"clock","x":0,"y":0,"w":1,"h":1}]"#)[0];
        assert!(w.config.is_null());
    }
}
