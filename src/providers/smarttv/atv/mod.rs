//! Android TV Remote v2 — the protocol modern Android/Google TVs (incl. recent
//! Sony Bravias) use for remote-key control, replacing the legacy IRCC-over-IP
//! SOAP endpoint that those models no longer expose.
//!
//! Two TLS channels, authenticated by a self-signed **client certificate**:
//! - **port 6467 (pairing, one-time):** exchange certs, the TV shows a 6-digit
//!   code, the client proves it by a SHA-256 secret derived from both certs'
//!   public keys and the code; the client cert then becomes trusted.
//! - **port 6466 (remote):** a configure/set-active handshake, then
//!   `RemoteKeyInject{keycode, direction}` for keys (Android `KEYCODE_*`) and
//!   ping/pong keep-alives.
//!
//! This module is built in layers: [`wire`] is the protobuf codec; the keycode
//! map below translates Bifrost's vendor-neutral [`RemoteKey`] to Android codes.

pub(crate) mod client;
pub(crate) mod crypto;
pub(crate) mod messages;
pub(crate) mod wire;

use crate::models::remote::RemoteKey;

/// Android `KeyEvent` keycode for a Bifrost remote key — the integer the TV's
/// `RemoteKeyInject` expects. Values are the stable `android.view.KeyEvent`
/// constants.
pub(crate) fn android_keycode(key: RemoteKey) -> u32 {
    match key {
        RemoteKey::Up => 19,         // KEYCODE_DPAD_UP
        RemoteKey::Down => 20,       // KEYCODE_DPAD_DOWN
        RemoteKey::Left => 21,       // KEYCODE_DPAD_LEFT
        RemoteKey::Right => 22,      // KEYCODE_DPAD_RIGHT
        RemoteKey::Select => 23,     // KEYCODE_DPAD_CENTER
        RemoteKey::Back => 4,        // KEYCODE_BACK
        RemoteKey::Home => 3,        // KEYCODE_HOME
        RemoteKey::Menu => 82,       // KEYCODE_MENU
        RemoteKey::VolumeUp => 24,   // KEYCODE_VOLUME_UP
        RemoteKey::VolumeDown => 25, // KEYCODE_VOLUME_DOWN
        RemoteKey::Mute => 164,      // KEYCODE_VOLUME_MUTE
        RemoteKey::PlayPause => 85,  // KEYCODE_MEDIA_PLAY_PAUSE
        RemoteKey::Next => 87,       // KEYCODE_MEDIA_NEXT
        RemoteKey::Previous => 88,   // KEYCODE_MEDIA_PREVIOUS
        RemoteKey::Power => 26,      // KEYCODE_POWER
    }
}

/// Android `KeyEvent` keycode for a literal character, for typing into a focused
/// field by injecting key events (Android TVs expose no ScalarWeb text input).
/// Covers the search-query character set — letters (lower-cased; search is
/// case-insensitive), digits, space, and common punctuation. Unmapped characters
/// return `None` and are skipped.
pub(crate) fn char_keycode(c: char) -> Option<u32> {
    let c = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => 29 + (c as u32 - 'a' as u32), // KEYCODE_A..KEYCODE_Z
        '0'..='9' => 7 + (c as u32 - '0' as u32),  // KEYCODE_0..KEYCODE_9
        ' ' => 62,                                 // KEYCODE_SPACE
        '.' => 56,                                 // KEYCODE_PERIOD
        ',' => 55,                                 // KEYCODE_COMMA
        '-' => 69,                                 // KEYCODE_MINUS
        '\'' => 75,                                // KEYCODE_APOSTROPHE
        '@' => 77,                                 // KEYCODE_AT
        '/' => 76,                                 // KEYCODE_SLASH
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_keycodes_cover_the_search_charset() {
        assert_eq!(char_keycode('a'), Some(29)); // KEYCODE_A
        assert_eq!(char_keycode('z'), Some(54)); // KEYCODE_Z
        assert_eq!(char_keycode('A'), Some(29)); // upper-cased to 'a' (case-insensitive)
        assert_eq!(char_keycode('0'), Some(7)); // KEYCODE_0
        assert_eq!(char_keycode('9'), Some(16)); // KEYCODE_9
        assert_eq!(char_keycode(' '), Some(62)); // KEYCODE_SPACE
        assert_eq!(char_keycode('\''), Some(75)); // KEYCODE_APOSTROPHE
        assert_eq!(char_keycode('é'), None); // unmapped → skipped
        assert_eq!(char_keycode('#'), None);
    }

    #[test]
    fn keycodes_match_android_constants() {
        assert_eq!(android_keycode(RemoteKey::Select), 23);
        assert_eq!(android_keycode(RemoteKey::Up), 19);
        assert_eq!(android_keycode(RemoteKey::Home), 3);
        assert_eq!(android_keycode(RemoteKey::Mute), 164);
        assert_eq!(android_keycode(RemoteKey::Power), 26);
    }

    #[test]
    fn every_remote_key_maps_to_a_distinct_dpad_or_media_code() {
        use RemoteKey::*;
        let all = [
            Up, Down, Left, Right, Select, Back, Home, Menu, VolumeUp, VolumeDown, Mute, PlayPause,
            Next, Previous, Power,
        ];
        let codes: Vec<u32> = all.iter().map(|k| android_keycode(*k)).collect();
        let mut uniq = codes.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), codes.len(), "keycodes must be unique");
    }
}
