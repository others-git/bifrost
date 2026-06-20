//! Remote-control domain: a **virtual smart-remote** for TVs / streamers.
//!
//! Some devices (an Android TV, a streaming box) aren't usefully modelled as a
//! light, a speaker, or a plug — their control surface is a *remote*: a D-pad,
//! navigation/media keys, and the ability to launch apps. That's its own domain
//! so the model stays honest (it is neither media state nor on/off).
//!
//! The model is deliberately provider-agnostic. The frontend `BifrostRemote`
//! renders a fixed set of **canonical keys** ([`RemoteKey`]); each provider maps
//! those to its native command vocabulary (HA's Android TV Remote keycodes,
//! etc.). App launch takes a free-form `activity` (a Play Store package id *or* a
//! deep-link URL), because that's the lowest common denominator across remotes.
//!
//! A remote is **paired** to the TV it controls (the `media_player`/media device
//! that is the same physical box) by hardware id, so the UI can offer the remote
//! from the TV's control fly-out. Pairing lives at the service/DB layer.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A virtual remote control for one device (a TV / streamer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteDevice {
    pub id: Uuid,
    /// Stable provider-specific identifier (e.g. an HA `entity_id` like
    /// `remote.bedroom_tv`).
    pub provider_id: String,
    pub name: String,
    pub state: RemoteState,
    /// Normalized hardware identity for cross-provider de-dup **and TV pairing**
    /// (the remote and its `media_player` share one device → one `hw_id`). `None`
    /// when the provider can't supply one. See [`crate::providers::mac_hw_id`].
    #[serde(default)]
    pub hw_id: Option<String>,
}

/// Full remote state. Minimal by design: whether the device is on, and the
/// foreground app (a package id, e.g. `com.netflix.ninja`) when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RemoteState {
    pub on: bool,
    /// The foreground app's package id (HA `current_activity`), if reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_app: Option<String>,
    /// Whether the device is reachable by its provider (`None` = not reported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
    /// The device's network address, when the provider knows it (a TV's host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
}

/// The canonical remote keys `BifrostRemote` renders. Providers translate each
/// to their native command — keeping the UI and the API provider-independent.
/// Intentionally the *common* set; provider-specific extras (colour buttons,
/// channels) can be added later or sent as raw text via [`RemoteCommand::Text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKey {
    Up,
    Down,
    Left,
    Right,
    /// D-pad centre / OK / select.
    Select,
    Back,
    Home,
    Menu,
    VolumeUp,
    VolumeDown,
    Mute,
    PlayPause,
    Next,
    Previous,
    Power,
}

/// One action sent to a remote: a canonical key press, free text entry, an app
/// launch (package id or deep-link URL), or an explicit power state. The API
/// accepts exactly one of these per request (tagged union).
/// (No `Eq`: `hold_secs` is an `f32`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCommand {
    /// Press one canonical key. `hold_secs` requests a long-press if supported.
    Key {
        key: RemoteKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_secs: Option<f32>,
    },
    /// Type literal text into the focused field.
    Text { text: String },
    /// Launch an app by Play Store package id (`com.netflix.ninja`) or a
    /// deep-link URL (`https://www.youtube.com/watch?v=…`).
    LaunchApp { activity: String },
    /// Power the device on/off.
    Power { on: bool },
    /// Send a provider-native command by its opaque `token` (from
    /// [`RemoteCommandInfo`]) — the keys beyond the canonical set that a specific
    /// device exposes (number pad, colour buttons, Input, Guide, …). The provider
    /// interprets the token (a Bravia IRCC code, an HA `send_command` name, …).
    Native { token: String },
}

/// One entry in a remote's **expanded** command catalogue — a provider-native
/// command the device exposes beyond the canonical [`RemoteKey`] set. Sent back
/// as [`RemoteCommand::Native`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteCommandInfo {
    /// Human label (the device's own name for it, e.g. `"Num1"`, `"Red"`, `"Input"`).
    pub name: String,
    /// Opaque token replayed via `RemoteCommand::Native` to invoke it.
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_key_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RemoteKey::PlayPause).unwrap(),
            "\"play_pause\""
        );
        assert_eq!(
            serde_json::to_string(&RemoteKey::VolumeUp).unwrap(),
            "\"volume_up\""
        );
    }

    #[test]
    fn remote_command_is_a_tagged_union() {
        let key: RemoteCommand = serde_json::from_str(r#"{"key":{"key":"up"}}"#).unwrap();
        assert_eq!(
            key,
            RemoteCommand::Key {
                key: RemoteKey::Up,
                hold_secs: None
            }
        );

        let app: RemoteCommand =
            serde_json::from_str(r#"{"launch_app":{"activity":"com.netflix.ninja"}}"#).unwrap();
        assert_eq!(
            app,
            RemoteCommand::LaunchApp {
                activity: "com.netflix.ninja".into()
            }
        );

        let power: RemoteCommand = serde_json::from_str(r#"{"power":{"on":true}}"#).unwrap();
        assert_eq!(power, RemoteCommand::Power { on: true });
    }

    #[test]
    fn remote_state_omits_absent_optionals() {
        let s = RemoteState {
            on: true,
            ..Default::default()
        };
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"on":true}"#);
    }
}
