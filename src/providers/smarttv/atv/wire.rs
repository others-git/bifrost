//! Minimal protobuf wire-format codec for the Android TV Remote v2 protocol.
//!
//! The protocol uses only a handful of small messages, so rather than pull in
//! `prost` + a build-time `protoc`/`protox` step (awkward for the single static
//! musl binary), we hand-roll the few wire primitives we need: base-128 varints,
//! field tags, and the two wire types the protocol actually uses — varint
//! (type 0) and length-delimited (type 2). Every message on the TLS stream is
//! itself length-delimited (a varint byte count, then that many bytes), which
//! [`FrameReader`] reassembles from a byte stream that may split mid-message.

/// Protobuf wire types we support (the only two the ATV protocol uses).
pub const WIRE_VARINT: u8 = 0;
pub const WIRE_LEN: u8 = 2;

/// Append a base-128 varint.
pub fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

/// Append a field tag (`field_number << 3 | wire_type`).
pub fn put_tag(buf: &mut Vec<u8>, field: u32, wire: u8) {
    put_varint(buf, ((field as u64) << 3) | wire as u64);
}

/// Append a varint scalar field (`field: value`). A zero value still writes the
/// field — callers omit defaults themselves when proto3 semantics require it.
pub fn put_varint_field(buf: &mut Vec<u8>, field: u32, value: u64) {
    put_tag(buf, field, WIRE_VARINT);
    put_varint(buf, value);
}

/// Append a length-delimited field (`field: bytes`) — sub-messages, strings, and
/// byte blobs all use this.
pub fn put_bytes_field(buf: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_tag(buf, field, WIRE_LEN);
    put_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

/// Frame `msg` for the stream: a varint length prefix followed by the bytes.
pub fn frame(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + 4);
    put_varint(&mut out, msg.len() as u64);
    out.extend_from_slice(msg);
    out
}

/// Read a varint from `data` starting at `*pos`, advancing `*pos`. Returns
/// `None` if the bytes available don't yet contain a complete varint.
pub fn get_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift = 0;
    let mut i = *pos;
    loop {
        let byte = *data.get(i)?;
        // Guard against a malformed/oversized varint (>10 bytes overflows u64).
        if shift >= 64 {
            return None;
        }
        result |= ((byte & 0x7f) as u64) << shift;
        i += 1;
        if byte & 0x80 == 0 {
            *pos = i;
            return Some(result);
        }
        shift += 7;
    }
}

/// One decoded protobuf field: its number and raw value.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Varint(u32, u64),
    Bytes(u32, Vec<u8>),
}

/// Parse a complete protobuf message body into its fields. Unknown wire types
/// (3/4 groups, 5 fixed32, 1 fixed64) aren't used by this protocol; encountering
/// one means a corrupt/foreign message, so parsing stops with what was read.
pub fn parse_fields(data: &[u8]) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let Some(tag) = get_varint(data, &mut pos) else {
            break;
        };
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x7) as u8;
        match wire {
            WIRE_VARINT => match get_varint(data, &mut pos) {
                Some(v) => fields.push(Field::Varint(field, v)),
                None => break,
            },
            WIRE_LEN => {
                let Some(len) = get_varint(data, &mut pos) else {
                    break;
                };
                let end = pos + len as usize;
                if end > data.len() {
                    break;
                }
                fields.push(Field::Bytes(field, data[pos..end].to_vec()));
                pos = end;
            }
            _ => break,
        }
    }
    fields
}

/// First varint value for `field`, if present.
pub fn field_varint(fields: &[Field], field: u32) -> Option<u64> {
    fields.iter().find_map(|f| match f {
        Field::Varint(n, v) if *n == field => Some(*v),
        _ => None,
    })
}

/// First bytes value for `field`, if present.
pub fn field_bytes(fields: &[Field], field: u32) -> Option<&[u8]> {
    fields.iter().find_map(|f| match f {
        Field::Bytes(n, b) if *n == field => Some(b.as_slice()),
        _ => None,
    })
}

/// Reassembles length-delimited frames from a byte stream that may deliver a
/// frame across several reads (or several frames in one read).
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Feed freshly-read bytes into the buffer.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pop the next complete frame's payload, or `None` if one isn't buffered yet.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        let mut pos = 0;
        let len = get_varint(&self.buf, &mut pos)? as usize;
        if self.buf.len() < pos + len {
            return None; // length prefix arrived but not the whole body yet
        }
        let payload = self.buf[pos..pos + len].to_vec();
        self.buf.drain(..pos + len);
        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrips_across_byte_boundaries() {
        for v in [0u64, 1, 127, 128, 300, 16_384, u32::MAX as u64, u64::MAX] {
            let mut buf = Vec::new();
            put_varint(&mut buf, v);
            let mut pos = 0;
            assert_eq!(get_varint(&buf, &mut pos), Some(v));
            assert_eq!(pos, buf.len(), "consumed exactly the varint for {v}");
        }
    }

    #[test]
    fn get_varint_returns_none_on_truncation() {
        // 300 encodes as two bytes [0xAC, 0x02]; only the continuation byte present.
        let mut pos = 0;
        assert_eq!(get_varint(&[0xAC], &mut pos), None);
    }

    #[test]
    fn tag_encodes_field_and_wire_type() {
        let mut buf = Vec::new();
        put_tag(&mut buf, 2, WIRE_LEN);
        // field 2, wire 2 → (2<<3)|2 = 18
        assert_eq!(buf, vec![18]);
    }

    #[test]
    fn parse_fields_reads_varint_and_bytes() {
        let mut msg = Vec::new();
        put_varint_field(&mut msg, 1, 10); // status = 10
        put_bytes_field(&mut msg, 2, b"hi"); // payload bytes
        let fields = parse_fields(&msg);
        assert_eq!(field_varint(&fields, 1), Some(10));
        assert_eq!(field_bytes(&fields, 2), Some(&b"hi"[..]));
        assert_eq!(field_varint(&fields, 9), None);
    }

    #[test]
    fn parse_fields_handles_nested_message() {
        // Outer field 5 carries a sub-message with field 1 = 6.
        let mut inner = Vec::new();
        put_varint_field(&mut inner, 1, 6);
        let mut outer = Vec::new();
        put_bytes_field(&mut outer, 5, &inner);
        let fields = parse_fields(&outer);
        let sub = field_bytes(&fields, 5).unwrap();
        assert_eq!(field_varint(&parse_fields(sub), 1), Some(6));
    }

    #[test]
    fn parse_fields_stops_on_truncated_length_delim() {
        // Claims 4 bytes of payload but only provides 1 — must not panic/over-read.
        let bytes = vec![(2 << 3) | WIRE_LEN, 4, 0xAA];
        assert!(parse_fields(&bytes).is_empty());
    }

    #[test]
    fn frame_reader_reassembles_split_and_batched_frames() {
        let a = frame(b"alpha");
        let b = frame(b"beta");
        let mut r = FrameReader::default();
        // First half of frame A only → nothing complete yet.
        r.push(&a[..3]);
        assert_eq!(r.next_frame(), None);
        // Rest of A plus all of B in one push → both pop out in order.
        r.push(&a[3..]);
        r.push(&b);
        assert_eq!(r.next_frame().as_deref(), Some(&b"alpha"[..]));
        assert_eq!(r.next_frame().as_deref(), Some(&b"beta"[..]));
        assert_eq!(r.next_frame(), None);
    }
}
