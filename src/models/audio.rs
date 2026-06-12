//! Audio device domain models: receivers, speakers, and the state/command
//! vocabulary shared by every audio provider (Onkyo, Sonos, …).
//!
//! The shape deliberately mirrors the light models: a device carries a full
//! `AudioState` snapshot, while writes are sparse `AudioCommand`s — only the
//! fields present are sent to the device.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An audio endpoint a provider can control. For a receiver each listening
/// zone is its own device (`main`, `zone2`, …); for Sonos each player is one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: Uuid,
    /// Stable provider-specific identifier (e.g. eISCP zone, Sonos player UUID).
    pub provider_id: String,
    pub name: String,
    pub kind: AudioDeviceKind,
    pub capabilities: AudioCapabilities,
    pub state: AudioState,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioDeviceKind {
    Receiver,
    Speaker,
    Zone,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioCapabilities {
    /// Device can switch between physical/virtual inputs (receivers).
    pub sources: bool,
    /// Device exposes play/pause/skip transport control.
    pub transport: bool,
    /// Device reports track metadata (title/artist/album).
    pub now_playing: bool,
}

/// Full state snapshot, as returned by `get_state` and stored in
/// `audio_devices.last_state`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioState {
    pub power: bool,
    /// 0–100.
    pub volume: u8,
    pub mute: bool,
    /// Provider-normalised source name (e.g. "net", "tv", "bd"). None when
    /// unknown or the device has a fixed source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_playing: Option<NowPlaying>,
    /// Whether the device answered its provider (None = not reported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NowPlaying {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_state: Option<PlayState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
}

/// A sparse write: only the fields present are applied, in a provider-defined
/// order (power first, so "power on + volume 40" works from standby).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<bool>,
    /// 0–100; providers clamp to their native scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// Source name as reported in `AudioState::source`, or a provider-native
    /// raw code (e.g. Onkyo hex like "2B").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportCmd>,
}

impl AudioCommand {
    pub fn is_empty(&self) -> bool {
        self.power.is_none()
            && self.volume.is_none()
            && self.mute.is_none()
            && self.source.is_none()
            && self.transport.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportCmd {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// Toggle play/pause.
    Toggle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_command_is_empty_only_when_all_fields_absent() {
        assert!(AudioCommand::default().is_empty());
        let cmd = AudioCommand {
            volume: Some(30),
            ..Default::default()
        };
        assert!(!cmd.is_empty());
    }

    #[test]
    fn audio_state_serializes_without_optional_noise() {
        let s = AudioState {
            power: true,
            volume: 42,
            mute: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"volume\":42"));
        assert!(!json.contains("now_playing"));
        assert!(!json.contains("source"));
    }

    #[test]
    fn audio_command_roundtrips_through_json() {
        let cmd = AudioCommand {
            power: Some(true),
            volume: Some(55),
            mute: Some(false),
            source: Some("net".into()),
            transport: Some(TransportCmd::Play),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: AudioCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(back.power, Some(true));
        assert_eq!(back.volume, Some(55));
        assert_eq!(back.source.as_deref(), Some("net"));
        assert_eq!(back.transport, Some(TransportCmd::Play));
    }

    #[test]
    fn transport_cmd_uses_lowercase_wire_names() {
        assert_eq!(
            serde_json::to_string(&TransportCmd::Previous).unwrap(),
            "\"previous\""
        );
        assert_eq!(
            serde_json::from_str::<TransportCmd>("\"toggle\"").unwrap(),
            TransportCmd::Toggle
        );
    }
}
