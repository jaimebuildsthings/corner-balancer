//! A small, crash-safe record format for settings kept in flash.
//!
//! The firmware keeps two slots (one flash sector each) and always writes the
//! slot *not* holding the newest record. If power fails mid-write, the other
//! slot still holds the previous good record, and the torn one fails its CRC.

/// Bytes added around the payload: magic, sequence number, length, CRC.
pub const OVERHEAD: usize = 4 + 4 + 2 + 4;

const MAGIC: u32 = u32::from_le_bytes(*b"CBRC");

/// Writes `payload` as a record into `out`, returning the bytes used,
/// or `None` if `out` is too small or the payload exceeds 65,535 bytes.
pub fn encode(seq: u32, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let len = u16::try_from(payload.len()).ok()?;
    let total = OVERHEAD + payload.len();
    let out = out.get_mut(..total)?;
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&seq.to_le_bytes());
    out[8..10].copy_from_slice(&len.to_le_bytes());
    out[10..10 + payload.len()].copy_from_slice(payload);
    let crc = crc32(&out[..total - 4]);
    out[total - 4..].copy_from_slice(&crc.to_le_bytes());
    Some(total)
}

/// Parses a record at the start of `bytes` (which may be a whole sector).
/// Returns its sequence number and payload, or `None` if the slot is erased,
/// foreign or corrupt.
pub fn decode(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if word(0)? != MAGIC {
        return None;
    }
    let seq = word(4)?;
    let len = usize::from(u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?));
    let body_end = 10 + len;
    if word(body_end)? != crc32(&bytes[..body_end]) {
        return None;
    }
    Some((seq, &bytes[10..body_end]))
}

/// Picks the newest valid record among the slots.
/// Returns the slot index, sequence number and payload.
pub fn newest<'a>(slots: &[&'a [u8]]) -> Option<(usize, u32, &'a [u8])> {
    slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| decode(s).map(|(seq, payload)| (i, seq, payload)))
        .max_by_key(|&(_, seq, _)| seq)
}

/// CRC-32 (IEEE 802.3, as used by zlib), bitwise to avoid a lookup table.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn round_trip_in_a_sector() {
        let mut sector = [0xFFu8; 4096];
        let n = encode(7, b"hello", &mut sector).unwrap();
        assert_eq!(n, OVERHEAD + 5);
        assert_eq!(decode(&sector), Some((7, &b"hello"[..])));
    }

    #[test]
    fn empty_payload() {
        let mut buf = [0u8; OVERHEAD];
        encode(1, &[], &mut buf).unwrap();
        assert_eq!(decode(&buf), Some((1, &[][..])));
    }

    #[test]
    fn rejects_erased_short_and_corrupt() {
        assert_eq!(decode(&[0xFF; 64]), None);
        assert_eq!(decode(&[]), None);
        let mut buf = [0u8; 64];
        let n = encode(3, b"abc", &mut buf).unwrap();
        assert_eq!(decode(&buf[..n - 1]), None);
        buf[11] ^= 0x01;
        assert_eq!(decode(&buf), None);
    }

    #[test]
    fn rejects_bad_length() {
        let mut buf = [0u8; 64];
        encode(3, b"abc", &mut buf).unwrap();
        buf[8] = 200;
        assert_eq!(decode(&buf), None);
    }

    #[test]
    fn encode_needs_room() {
        let mut small = [0u8; OVERHEAD + 2];
        assert_eq!(encode(1, b"abc", &mut small), None);
    }

    #[test]
    fn newest_prefers_higher_sequence_and_skips_torn_slots() {
        let (mut a, mut b) = ([0xFFu8; 64], [0xFFu8; 64]);
        assert_eq!(newest(&[&a, &b]), None);
        encode(1, b"old", &mut a).unwrap();
        assert_eq!(newest(&[&a, &b]), Some((0, 1, &b"old"[..])));
        encode(2, b"new", &mut b).unwrap();
        assert_eq!(newest(&[&a, &b]), Some((1, 2, &b"new"[..])));
        // A torn write of slot B falls back to slot A.
        b[12] ^= 0xFF;
        assert_eq!(newest(&[&a, &b]), Some((0, 1, &b"old"[..])));
    }
}
