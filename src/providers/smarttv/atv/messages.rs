//! Builders and parsers for the two Android TV Remote v2 message families, on
//! top of the [`super::wire`] codec. Field numbers and enum values are taken
//! verbatim from the canonical `pairingmessage.proto` / `remotemessage.proto`.

use super::wire;

// ── Pairing channel (port 6467) ─────────────────────────────────────────────
// PairingMessage: protocol_version=1, status=2, pairing_request=10,
// pairing_request_ack=11, pairing_option=20, pairing_configuration=30,
// pairing_configuration_ack=31, pairing_secret=40, pairing_secret_ack=41.

const PROTOCOL_VERSION: u64 = 2;
const STATUS_OK: u64 = 200;
const ROLE_TYPE_INPUT: u64 = 1;
const ENCODING_TYPE_HEXADECIMAL: u64 = 3;
/// Code length the TV displays (6 hex symbols).
const SYMBOL_LENGTH: u64 = 6;

/// Wrap a pairing payload (`field`/`body`) in a `PairingMessage` envelope with
/// `protocol_version` + `STATUS_OK`.
fn pairing_envelope(field: u32, body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::new();
    wire::put_varint_field(&mut msg, 1, PROTOCOL_VERSION);
    wire::put_varint_field(&mut msg, 2, STATUS_OK);
    wire::put_bytes_field(&mut msg, field, body);
    msg
}

/// `PairingRequest{ service_name, client_name }` (field 10).
pub fn pairing_request(client_name: &str) -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_bytes_field(&mut body, 1, b"atvremote"); // service_name
    wire::put_bytes_field(&mut body, 2, client_name.as_bytes()); // client_name
    pairing_envelope(10, &body)
}

/// A `PairingEncoding{ type=HEXADECIMAL, symbol_length=6 }` sub-message.
fn hex_encoding() -> Vec<u8> {
    let mut enc = Vec::new();
    wire::put_varint_field(&mut enc, 1, ENCODING_TYPE_HEXADECIMAL);
    wire::put_varint_field(&mut enc, 2, SYMBOL_LENGTH);
    enc
}

/// `PairingOption{ input_encodings=[hex], preferred_role=INPUT }` (field 20).
pub fn pairing_option() -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_bytes_field(&mut body, 1, &hex_encoding()); // input_encodings
    wire::put_varint_field(&mut body, 3, ROLE_TYPE_INPUT); // preferred_role
    pairing_envelope(20, &body)
}

/// `PairingConfiguration{ encoding=hex, client_role=INPUT }` (field 30).
pub fn pairing_configuration() -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_bytes_field(&mut body, 1, &hex_encoding()); // encoding
    wire::put_varint_field(&mut body, 2, ROLE_TYPE_INPUT); // client_role
    pairing_envelope(30, &body)
}

/// `PairingSecret{ secret }` (field 40).
pub fn pairing_secret(secret: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_bytes_field(&mut body, 1, secret);
    pairing_envelope(40, &body)
}

/// `RemoteVoiceBegin{session_id}` (field 30) — open a voice session; the TV
/// pops its Assistant overlay and starts listening. `package_name` is only for
/// the reverse direction (TV→device on KEYCODE_SEARCH), omitted when we send.
pub fn remote_voice_begin(session_id: i32) -> Vec<u8> {
    let mut inner = Vec::new();
    wire::put_varint_field(&mut inner, 1, session_id as u64);
    let mut m = Vec::new();
    wire::put_bytes_field(&mut m, 30, &inner);
    m
}

/// `RemoteVoicePayload{session_id, samples}` (field 31) — one chunk of audio
/// (16-bit PCM, 8 kHz mono per the proto), ≤20 KB per message.
pub fn remote_voice_payload(session_id: i32, samples: &[u8]) -> Vec<u8> {
    let mut inner = Vec::new();
    wire::put_varint_field(&mut inner, 1, session_id as u64);
    wire::put_bytes_field(&mut inner, 2, samples);
    let mut m = Vec::new();
    wire::put_bytes_field(&mut m, 31, &inner);
    m
}

/// `RemoteVoiceEnd{session_id}` (field 32) — close the voice session; the TV
/// runs the utterance through its Assistant.
pub fn remote_voice_end(session_id: i32) -> Vec<u8> {
    let mut inner = Vec::new();
    wire::put_varint_field(&mut inner, 1, session_id as u64);
    let mut m = Vec::new();
    wire::put_bytes_field(&mut m, 32, &inner);
    m
}

/// A `RemoteAppLinkLaunchRequest{app_link}` (field 90) — opens an app by
/// Play Store package id or deep link, the ATV-native launch path.
pub fn remote_app_link_launch(app_link: &str) -> Vec<u8> {
    let mut inner = Vec::new();
    wire::put_bytes_field(&mut inner, 1, app_link.as_bytes());
    let mut m = Vec::new();
    wire::put_bytes_field(&mut m, 90, &inner);
    m
}

/// The kind of pairing message the TV sent back, with its status code.
#[derive(Debug, PartialEq)]
pub enum PairingIn {
    RequestAck,
    Options,
    ConfigurationAck,
    SecretAck,
    /// A non-OK status, or an unrecognised payload.
    Other(u64),
}

/// Classify an incoming `PairingMessage` body.
pub fn parse_pairing(body: &[u8]) -> PairingIn {
    let fields = wire::parse_fields(body);
    let status = wire::field_varint(&fields, 2).unwrap_or(0);
    if status != STATUS_OK {
        return PairingIn::Other(status);
    }
    if wire::field_bytes(&fields, 11).is_some() {
        PairingIn::RequestAck
    } else if wire::field_bytes(&fields, 20).is_some() {
        PairingIn::Options
    } else if wire::field_bytes(&fields, 31).is_some() {
        PairingIn::ConfigurationAck
    } else if wire::field_bytes(&fields, 41).is_some() {
        PairingIn::SecretAck
    } else {
        PairingIn::Other(status)
    }
}

// ── Remote channel (port 6466) ──────────────────────────────────────────────
// RemoteMessage: remote_configure=1, remote_set_active=2, remote_ping_request=8,
// remote_ping_response=9, remote_key_inject=10.

/// Capability/feature code both `RemoteConfigure.code1` and `RemoteSetActive`
/// carry — the value the reference clients send.
const ACTIVE_FEATURES: u64 = 622;
/// `RemoteDirection.SHORT` — a normal tap (vs START_LONG/END_LONG).
const DIRECTION_SHORT: u64 = 3;

/// `RemoteConfigure{ code1, device_info{ unknown1, unknown2, package_name,
/// app_version } }` (field 1) — the client's reply to the TV's configure.
pub fn remote_configure() -> Vec<u8> {
    let mut info = Vec::new();
    wire::put_varint_field(&mut info, 3, 1); // unknown1
    wire::put_bytes_field(&mut info, 4, b"1"); // unknown2
    wire::put_bytes_field(&mut info, 5, b"atvremote"); // package_name
    wire::put_bytes_field(&mut info, 6, b"1.0.0"); // app_version

    let mut cfg = Vec::new();
    wire::put_varint_field(&mut cfg, 1, ACTIVE_FEATURES); // code1
    wire::put_bytes_field(&mut cfg, 2, &info); // device_info

    let mut msg = Vec::new();
    wire::put_bytes_field(&mut msg, 1, &cfg);
    msg
}

/// `RemoteSetActive{ active }` (field 2).
pub fn remote_set_active() -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_varint_field(&mut body, 1, ACTIVE_FEATURES);
    let mut msg = Vec::new();
    wire::put_bytes_field(&mut msg, 2, &body);
    msg
}

/// `RemotePingResponse{ val1 }` (field 9) — echo the request's value.
pub fn remote_ping_response(val1: u64) -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_varint_field(&mut body, 1, val1);
    let mut msg = Vec::new();
    wire::put_bytes_field(&mut msg, 9, &body);
    msg
}

/// `RemoteKeyInject{ key_code, direction=SHORT }` (field 10).
pub fn remote_key_inject(key_code: u32) -> Vec<u8> {
    let mut body = Vec::new();
    wire::put_varint_field(&mut body, 1, key_code as u64); // key_code
    wire::put_varint_field(&mut body, 2, DIRECTION_SHORT); // direction
    let mut msg = Vec::new();
    wire::put_bytes_field(&mut msg, 10, &body);
    msg
}

/// What the TV sent on the remote channel (only the parts we react to).
#[derive(Debug, PartialEq)]
pub enum RemoteIn {
    Configure,
    SetActive,
    /// A ping carrying the value we must echo in a `RemotePingResponse`.
    Ping(u64),
    /// The foreground app changed — `remote_ime_key_inject.app_info.app_package`
    /// (the TV pushes one whenever a new app takes the screen).
    CurrentApp(String),
    /// Absolute device volume — `remote_set_volume_level`.
    Volume {
        level: u32,
        max: u32,
        muted: bool,
    },
    /// Screen state — `remote_start.started` (true = on, false = sleeping).
    Started(bool),
    Other,
}

/// Classify an incoming `RemoteMessage` body. Field numbers are the canonical
/// `remotemessage.proto` tags: configure=1, set_active=2, ping_request=8,
/// ime_key_inject=20 (app_info=1 → app_package=12), start=40,
/// set_volume_level=50 (volume_max=6, volume_level=7, volume_muted=8).
pub fn parse_remote(body: &[u8]) -> RemoteIn {
    let fields = wire::parse_fields(body);
    if wire::field_bytes(&fields, 1).is_some() {
        RemoteIn::Configure
    } else if wire::field_bytes(&fields, 2).is_some() {
        RemoteIn::SetActive
    } else if let Some(ping) = wire::field_bytes(&fields, 8) {
        let val1 = wire::field_varint(&wire::parse_fields(ping), 1).unwrap_or(0);
        RemoteIn::Ping(val1)
    } else if let Some(ime) = wire::field_bytes(&fields, 20) {
        let app = wire::field_bytes(&wire::parse_fields(ime), 1)
            .map(wire::parse_fields)
            .and_then(|info| {
                wire::field_bytes(&info, 12).map(|b| String::from_utf8_lossy(b).into_owned())
            });
        match app {
            Some(pkg) if !pkg.is_empty() => RemoteIn::CurrentApp(pkg),
            _ => RemoteIn::Other,
        }
    } else if let Some(start) = wire::field_bytes(&fields, 40) {
        let started = wire::field_varint(&wire::parse_fields(start), 1) == Some(1);
        RemoteIn::Started(started)
    } else if let Some(vol) = wire::field_bytes(&fields, 50) {
        let f = wire::parse_fields(vol);
        RemoteIn::Volume {
            // proto3 omits zero fields — an absent level really is 0.
            level: wire::field_varint(&f, 7).unwrap_or(0) as u32,
            max: wire::field_varint(&f, 6).unwrap_or(0) as u32,
            muted: wire::field_varint(&f, 8) == Some(1),
        }
    } else {
        RemoteIn::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A RemoteMessage body carrying `inner` as message-typed `field`.
    fn msg(field: u32, inner: &[u8]) -> Vec<u8> {
        let mut m = Vec::new();
        wire::put_bytes_field(&mut m, field, inner);
        m
    }

    #[test]
    fn voice_messages_carry_session_and_samples() {
        // begin: field 30 { session_id=1 }
        let f = wire::parse_fields(&remote_voice_begin(7));
        let begin = wire::field_bytes(&f, 30).expect("voice_begin");
        assert_eq!(wire::field_varint(&wire::parse_fields(begin), 1), Some(7));

        // payload: field 31 { session_id, samples }
        let f = wire::parse_fields(&remote_voice_payload(7, &[1, 2, 3, 4]));
        let pay = wire::parse_fields(wire::field_bytes(&f, 31).expect("voice_payload"));
        assert_eq!(wire::field_varint(&pay, 1), Some(7));
        assert_eq!(wire::field_bytes(&pay, 2), Some(&[1u8, 2, 3, 4][..]));

        // end: field 32 { session_id }
        let f = wire::parse_fields(&remote_voice_end(7));
        let end = wire::field_bytes(&f, 32).expect("voice_end");
        assert_eq!(wire::field_varint(&wire::parse_fields(end), 1), Some(7));
    }

    #[test]
    fn parse_current_app_from_ime_key_inject() {
        // remote_ime_key_inject(20) { app_info(1) { app_package(12) } }
        let mut info = Vec::new();
        wire::put_varint_field(&mut info, 1, 42); // counter noise
        wire::put_bytes_field(&mut info, 12, b"com.netflix.ninja");
        let mut ime = Vec::new();
        wire::put_bytes_field(&mut ime, 1, &info);
        assert_eq!(
            parse_remote(&msg(20, &ime)),
            RemoteIn::CurrentApp("com.netflix.ninja".into())
        );
        // No package → not an app change.
        let mut empty = Vec::new();
        wire::put_bytes_field(&mut empty, 1, &[]);
        assert_eq!(parse_remote(&msg(20, &empty)), RemoteIn::Other);
    }

    #[test]
    fn parse_volume_level_with_max_and_mute() {
        let mut vol = Vec::new();
        wire::put_bytes_field(&mut vol, 3, b"BRAVIA"); // player_model noise
        wire::put_varint_field(&mut vol, 6, 100);
        wire::put_varint_field(&mut vol, 7, 28);
        wire::put_varint_field(&mut vol, 8, 0);
        assert_eq!(
            parse_remote(&msg(50, &vol)),
            RemoteIn::Volume {
                level: 28,
                max: 100,
                muted: false
            }
        );
        // proto3 zero-omission: an empty body is volume 0 of max 0, unmuted.
        assert_eq!(
            parse_remote(&msg(50, &[])),
            RemoteIn::Volume {
                level: 0,
                max: 0,
                muted: false
            }
        );
    }

    #[test]
    fn parse_screen_state_from_remote_start() {
        let mut on = Vec::new();
        wire::put_varint_field(&mut on, 1, 1);
        assert_eq!(parse_remote(&msg(40, &on)), RemoteIn::Started(true));
        // started=false is omitted in proto3 → an empty RemoteStart means off.
        assert_eq!(parse_remote(&msg(40, &[])), RemoteIn::Started(false));
    }

    /// A pairing message we build round-trips through the field parser with the
    /// right envelope (version + OK status) and payload field.
    #[test]
    fn pairing_request_has_envelope_and_payload() {
        let msg = pairing_request("Bifrost");
        let f = wire::parse_fields(&msg);
        assert_eq!(wire::field_varint(&f, 1), Some(PROTOCOL_VERSION));
        assert_eq!(wire::field_varint(&f, 2), Some(STATUS_OK));
        let body = wire::field_bytes(&f, 10).expect("pairing_request payload");
        let inner = wire::parse_fields(body);
        assert_eq!(wire::field_bytes(&inner, 1), Some(&b"atvremote"[..]));
        assert_eq!(wire::field_bytes(&inner, 2), Some(&b"Bifrost"[..]));
    }

    #[test]
    fn option_and_configuration_carry_hex_encoding() {
        for (msg, payload_field, role_field) in [
            (pairing_option(), 20u32, 3u32),
            (pairing_configuration(), 30, 2),
        ] {
            let f = wire::parse_fields(&msg);
            let body = wire::field_bytes(&f, payload_field).unwrap();
            let inner = wire::parse_fields(body);
            let enc = wire::field_bytes(&inner, 1).unwrap();
            let ef = wire::parse_fields(enc);
            assert_eq!(wire::field_varint(&ef, 1), Some(ENCODING_TYPE_HEXADECIMAL));
            assert_eq!(wire::field_varint(&ef, 2), Some(SYMBOL_LENGTH));
            assert_eq!(
                wire::field_varint(&inner, role_field),
                Some(ROLE_TYPE_INPUT)
            );
        }
    }

    #[test]
    fn parse_pairing_classifies_by_payload_field() {
        // Build a fake server ack (status OK + field 11) and confirm classification.
        let ack = pairing_envelope(11, b"");
        assert_eq!(parse_pairing(&ack), PairingIn::RequestAck);
        let opts = pairing_envelope(20, &hex_encoding());
        assert_eq!(parse_pairing(&opts), PairingIn::Options);
        let cfg_ack = pairing_envelope(31, b"");
        assert_eq!(parse_pairing(&cfg_ack), PairingIn::ConfigurationAck);
    }

    #[test]
    fn parse_pairing_reports_non_ok_status() {
        // status=402 (STATUS_BAD_SECRET), no recognised payload.
        let mut msg = Vec::new();
        wire::put_varint_field(&mut msg, 2, 402);
        assert_eq!(parse_pairing(&msg), PairingIn::Other(402));
    }

    #[test]
    fn remote_key_inject_carries_keycode_and_short_direction() {
        let msg = remote_key_inject(19); // DPAD_UP
        let f = wire::parse_fields(&msg);
        let body = wire::field_bytes(&f, 10).expect("key_inject payload");
        let inner = wire::parse_fields(body);
        assert_eq!(wire::field_varint(&inner, 1), Some(19));
        assert_eq!(wire::field_varint(&inner, 2), Some(DIRECTION_SHORT));
    }

    #[test]
    fn parse_remote_extracts_ping_value() {
        // RemoteMessage{ remote_ping_request(8){ val1=42 } }.
        let mut ping = Vec::new();
        wire::put_varint_field(&mut ping, 1, 42);
        let mut msg = Vec::new();
        wire::put_bytes_field(&mut msg, 8, &ping);
        assert_eq!(parse_remote(&msg), RemoteIn::Ping(42));

        assert_eq!(parse_remote(&remote_configure()), RemoteIn::Configure);
        assert_eq!(parse_remote(&remote_set_active()), RemoteIn::SetActive);
    }
}
