//! Media device domain models: receivers, speakers, and the state/command
//! vocabulary shared by every media provider (Onkyo, Sonos, …).
//!
//! The shape deliberately mirrors the light models: a device carries a full
//! `MediaState` snapshot, while writes are sparse `MediaCommand`s — only the
//! fields present are sent to the device.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An media endpoint a provider can control. For a receiver each listening
/// zone is its own device (`main`, `zone2`, …); for Sonos each player is one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDevice {
    pub id: Uuid,
    /// Stable provider-specific identifier (e.g. eISCP zone, Sonos player UUID).
    pub provider_id: String,
    pub name: String,
    pub kind: MediaDeviceKind,
    pub capabilities: MediaCapabilities,
    pub state: MediaState,
    /// Normalized hardware identity for cross-provider de-dup (see
    /// [`crate::providers::mac_hw_id`]); `None` when the provider can't supply one.
    #[serde(default)]
    pub hw_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaDeviceKind {
    Receiver,
    Speaker,
    /// A television / display whose media Bifrost controls (HA `media_player`
    /// with `device_class: "tv"`). Kept distinct from a plain speaker/receiver
    /// so the UI can identify it as a TV.
    Tv,
    Zone,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaCapabilities {
    /// Device can switch between physical/virtual inputs (receivers).
    pub sources: bool,
    /// Device exposes play/pause/skip transport control.
    pub transport: bool,
    /// Device reports track metadata (title/artist/album).
    pub now_playing: bool,
    /// Device exposes saved favorites/presets the user can start playing
    /// (e.g. Sonos Favorites). `#[serde(default)]` keeps state stored before
    /// this field existed deserializable — it reads back as `false` until the
    /// next discovery refreshes it.
    #[serde(default)]
    pub favorites: bool,
    /// Device can be joined into/out of a provider-native playback group of
    /// speakers that play in sync (e.g. Sonos grouping), independent of Bifrost
    /// Rooms. `#[serde(default)]` keeps older stored state deserializable.
    #[serde(default)]
    pub grouping: bool,
}

/// A saved favorite/preset on a provider (e.g. a Sonos Favorite) that the user
/// can start playing by reference — no search or accounts involved. `id` is
/// provider-native and opaque; the client passes it back verbatim to play.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaFavorite {
    pub id: String,
    pub title: String,
    /// Optional secondary line (service name, description, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
}

/// Full state snapshot, as returned by `get_state` and stored in
/// `media_devices.last_state`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaState {
    pub power: bool,
    /// 0–100.
    pub volume: u8,
    pub mute: bool,
    /// Provider-normalised source name (e.g. "net", "tv", "bd"). None when
    /// unknown or the device has a fixed source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Selectable sources/inputs the device exposes — receiver inputs, or the
    /// apps on a smart TV (HA `source_list`). Switch to one by sending its name
    /// as `MediaCommand::source`. Empty when the device doesn't enumerate them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_list: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_playing: Option<NowPlaying>,
    /// Whether the device answered its provider (None = not reported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
    /// When this device is part of a live multi-speaker sync group, the
    /// provider-native id of the group's *coordinator*. Devices sharing a
    /// `group_coordinator` are playing in sync; the UI derives a single grouped
    /// control from them (no group device is stored). `None` = standalone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_coordinator: Option<String>,
    /// The device's network address, when the provider knows it (a Sonos player's
    /// IP, a TV's host, …). Read-only status surfaced on the Devices page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
pub struct MediaCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<bool>,
    /// 0–100; providers clamp to their native scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// Source name as reported in `MediaState::source`, or a provider-native
    /// raw code (e.g. Onkyo hex like "2B").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportCmd>,
}

impl MediaCommand {
    pub fn is_empty(&self) -> bool {
        self.power.is_none()
            && self.volume.is_none()
            && self.mute.is_none()
            && self.source.is_none()
            && self.transport.is_none()
    }

    /// Split a command for a source device bound to a receiver (M22). The
    /// receiver is the volume authority, so `volume`/`mute` route to it while
    /// `power`/`source`/`transport` stay on the source. Powering the source
    /// **on** also wakes the receiver and (when `receiver_source` is set)
    /// switches it to that input — so "turn on the TV" lands sound on the right
    /// receiver input. Powering *off* is not propagated: a receiver may serve
    /// several bound sources (many-to-one), so one source going dark mustn't
    /// silence it. Returns `(source_cmd, receiver_cmd)`.
    pub fn split_for_receiver(
        &self,
        receiver_source: Option<&str>,
    ) -> (MediaCommand, MediaCommand) {
        let source_cmd = MediaCommand {
            power: self.power,
            source: self.source.clone(),
            transport: self.transport,
            volume: None,
            mute: None,
        };
        let mut receiver_cmd = MediaCommand {
            volume: self.volume,
            mute: self.mute,
            ..Default::default()
        };
        if self.power == Some(true) {
            receiver_cmd.power = Some(true);
            receiver_cmd.source = receiver_source.map(str::to_string);
        }
        (source_cmd, receiver_cmd)
    }
}

/// A full-state update pushed by an media device (e.g. Onkyo's unsolicited
/// eISCP echoes). Push sources accumulate state internally and always emit
/// complete snapshots, so consumers replace rather than merge.
#[derive(Debug, Clone, Serialize)]
pub struct MediaEvent {
    /// Provider-native device id (e.g. `main`).
    pub device_id: String,
    pub state: MediaState,
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
    fn media_command_is_empty_only_when_all_fields_absent() {
        assert!(MediaCommand::default().is_empty());
        let cmd = MediaCommand {
            volume: Some(30),
            ..Default::default()
        };
        assert!(!cmd.is_empty());
    }

    #[test]
    fn media_state_serializes_without_optional_noise() {
        let s = MediaState {
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
    fn media_command_roundtrips_through_json() {
        let cmd = MediaCommand {
            power: Some(true),
            volume: Some(55),
            mute: Some(false),
            source: Some("net".into()),
            transport: Some(TransportCmd::Play),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: MediaCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(back.power, Some(true));
        assert_eq!(back.volume, Some(55));
        assert_eq!(back.source.as_deref(), Some("net"));
        assert_eq!(back.transport, Some(TransportCmd::Play));
    }

    #[test]
    fn split_for_receiver_routes_volume_and_mute_to_receiver() {
        let cmd = MediaCommand {
            volume: Some(40),
            mute: Some(true),
            transport: Some(TransportCmd::Play),
            ..Default::default()
        };
        let (source, receiver) = cmd.split_for_receiver(Some("Game"));
        // Volume/mute leave the source and land on the receiver.
        assert_eq!(source.volume, None);
        assert_eq!(source.mute, None);
        assert_eq!(source.transport, Some(TransportCmd::Play));
        assert_eq!(receiver.volume, Some(40));
        assert_eq!(receiver.mute, Some(true));
        // No power-on, so the receiver input is left alone.
        assert_eq!(receiver.power, None);
        assert_eq!(receiver.source, None);
    }

    #[test]
    fn split_for_receiver_power_on_wakes_receiver_and_switches_input() {
        let cmd = MediaCommand {
            power: Some(true),
            ..Default::default()
        };
        let (source, receiver) = cmd.split_for_receiver(Some("BD/DVD"));
        assert_eq!(source.power, Some(true));
        assert_eq!(receiver.power, Some(true));
        assert_eq!(receiver.source.as_deref(), Some("BD/DVD"));
    }

    #[test]
    fn split_for_receiver_power_off_is_not_propagated_to_receiver() {
        // Many sources may share one receiver, so one going dark mustn't silence it.
        let cmd = MediaCommand {
            power: Some(false),
            ..Default::default()
        };
        let (source, receiver) = cmd.split_for_receiver(Some("Game"));
        assert_eq!(source.power, Some(false));
        assert!(receiver.is_empty());
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
