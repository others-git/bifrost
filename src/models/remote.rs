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

/// Friendly name for a known Play Store package. Matches by **brand keyword**,
/// because the same app ships under many package ids across TV makers and regions
/// (`com.hulu.plus`, `com.hulu.livingroomplus`, …) — exact matching missed those.
/// Order matters: more specific keywords first. Unknown packages are prettified.
pub fn app_display_name(package: &str) -> String {
    let p = package.to_ascii_lowercase();
    // (keyword, friendly name). Keywords are checked in order against the
    // lowercased package id; the first contained match wins.
    const KNOWN: &[(&str, &str)] = &[
        ("youtube.tvkids", "YouTube Kids"),
        ("youtube", "YouTube"),
        ("netflix", "Netflix"),
        ("amazonvideo", "Prime Video"),
        ("amazon.avod", "Prime Video"),
        ("primevideo", "Prime Video"),
        ("disneyplus", "Disney+"),
        ("hulu", "Hulu"),
        ("hbo", "Max"),
        ("wbd.stream", "Max"),
        ("spotify", "Spotify"),
        ("plexapp", "Plex"),
        ("kodi", "Kodi"),
        ("twitch", "Twitch"),
        ("appletv", "Apple TV"),
        ("apple.atve", "Apple TV"),
        ("peacock", "Peacock"),
        ("paramount", "Paramount+"),
        ("crunchyroll", "Crunchyroll"),
        ("tubitv", "Tubi"),
        ("pluto", "Pluto TV"),
        ("sling", "Sling TV"),
        ("pandora", "Pandora"),
        ("vudu", "Vudu"),
        ("dreamx", "Screensaver"),
    ];
    for (kw, name) in KNOWN {
        if p.contains(kw) {
            return name.to_string();
        }
    }
    prettify_package(package)
}

/// Best-effort readable name for an unknown package id — capitalize the vendor
/// segment (`com.foobar.tv` → "Foobar"), falling back to the id if it's not a
/// dotted package. Better than showing `com.foobar.tv` raw.
pub fn prettify_package(package: &str) -> String {
    if package.contains("://") || !package.contains('.') {
        return package.to_string();
    }
    let parts: Vec<&str> = package.split('.').filter(|s| !s.is_empty()).collect();
    // The vendor (2nd) segment is usually the brand; fall back to the last.
    let seg = parts
        .get(1)
        .filter(|s| s.len() > 2)
        .or_else(|| parts.last())
        .copied()
        .unwrap_or(package);
    let mut chars = seg.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => package.to_string(),
    }
}

/// Percent-encode a string for use as a URL query component (RFC 3986
/// unreserved characters kept, spaces and everything else escaped).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A deep link that opens the app's own **search results for `query`** — the
/// native path from "(app, title)" to content on screen, with no per-app API
/// integration. Android App Links route these https URLs straight into the
/// installed app (`RemoteAppLinkLaunchRequest` / HA `remote.turn_on activity`
/// both accept them). Keyed by the same brand keywords as
/// [`app_display_name`], so any package variant of an app resolves. `None` =
/// no known template; callers fall back to opening the app.
pub fn app_search_deep_link(package: &str, query: &str) -> Option<String> {
    let p = package.to_ascii_lowercase();
    let q = url_encode(query.trim());
    // (keyword, template) — checked in order; more specific keywords first.
    const LINKS: &[(&str, &str)] = &[
        ("youtube.tvmusic", "https://music.youtube.com/search?q={q}"),
        (
            "youtube",
            "https://www.youtube.com/results?search_query={q}",
        ),
        ("netflix", "https://www.netflix.com/search?q={q}"),
        ("hulu", "https://www.hulu.com/search?q={q}"),
        (
            "amazonvideo",
            "https://www.primevideo.com/search?phrase={q}",
        ),
        (
            "amazon.avod",
            "https://www.primevideo.com/search?phrase={q}",
        ),
        ("primevideo", "https://www.primevideo.com/search?phrase={q}"),
        ("disneyplus", "https://www.disneyplus.com/search?q={q}"),
        ("hbo", "https://play.max.com/search?q={q}"),
        ("wbd.stream", "https://play.max.com/search?q={q}"),
        ("spotify", "https://open.spotify.com/search/{q}"),
        ("plexapp", "https://watch.plex.tv/search?query={q}"),
        ("twitch", "https://www.twitch.tv/search?term={q}"),
        ("appletv", "https://tv.apple.com/search?term={q}"),
        ("apple.atve", "https://tv.apple.com/search?term={q}"),
        ("peacock", "https://www.peacocktv.com/search?q={q}"),
        ("paramount", "https://www.paramountplus.com/search/?q={q}"),
        ("crunchyroll", "https://www.crunchyroll.com/search?q={q}"),
        ("tubitv", "https://tubitv.com/search/{q}"),
    ];
    for (kw, template) in LINKS {
        if p.contains(kw) {
            return Some(template.replace("{q}", &q));
        }
    }
    None
}

/// Foreground packages that aren't a user app on screen — the launcher, the
/// screensaver, system chrome. Now-playing clears rather than naming them.
/// The one rule every foreground-app source shares (the ATV push channel,
/// Home Assistant's `app_id`).
pub fn is_system_surface(package: &str) -> bool {
    let p = package.to_ascii_lowercase();
    [
        "launcher",
        "dream",
        "screensaver",
        "backdrop",
        "systemui",
        "inputmethod",
    ]
    .iter()
    .any(|kw| p.contains(kw))
}

/// One installed app from a TV's own catalog (`appControl.getApplicationList`):
/// the bare package id (the cross-source identity — the ATV push channel and
/// HA report foreground apps by package), the TV's display title, and the
/// vendor launch URI when the catalog provides one richer than the package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledApp {
    pub package: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
}

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RemoteCommandInfo {
    /// Human label (the device's own name for it, e.g. `"Num1"`, `"Red"`, `"Input"`).
    pub name: String,
    /// Opaque token replayed via `RemoteCommand::Native` to invoke it.
    pub token: String,
    /// Whether the user pinned this command as a favourite (overlaid by the
    /// service from `remote_command_pins`; always false as the provider sees it).
    #[serde(default)]
    pub pinned: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_links_resolve_by_brand_keyword_and_encode_the_title() {
        let link = app_search_deep_link("com.netflix.ninja", "bob's burgers").unwrap();
        assert_eq!(link, "https://www.netflix.com/search?q=bob%27s%20burgers");
        // Any package variant of the brand resolves (same rule as naming).
        assert!(
            app_search_deep_link("com.hulu.livingroomplus", "x")
                .unwrap()
                .starts_with("https://www.hulu.com/search")
        );
        assert!(
            app_search_deep_link("com.google.android.youtube.tv", "lofi beats")
                .unwrap()
                .contains("search_query=lofi%20beats")
        );
        // Unknown app → no template (caller opens the app instead).
        assert!(app_search_deep_link("com.example.obscure", "x").is_none());
    }

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(
            url_encode("Bob's Burgers & fries"),
            "Bob%27s%20Burgers%20%26%20fries"
        );
        assert_eq!(url_encode("plain-safe_1.0~x"), "plain-safe_1.0~x");
    }

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
