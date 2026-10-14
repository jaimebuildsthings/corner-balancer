//! A pad's reading as a fixed-size, versioned frame.

use crate::pad::{Corner, corner_from_byte, corner_to_byte};

/// One reading from one pad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadMsg {
    /// The corner the sending pad is assigned to, if any. (The pad itself is
    /// identified by the sender's MAC address, which ESP-NOW supplies.)
    pub corner: Option<Corner>,
    /// Increments per message; gaps reveal dropped frames.
    pub seq: u32,
    /// Weight after calibration and tare, kg.
    pub kg: f32,
    /// Raw HX711 count behind this reading.
    pub raw: i32,
    /// The reading has settled.
    pub stable: bool,
    /// The pad has a stored calibration (otherwise `kg` is in counts).
    pub calibrated: bool,
    /// Pad temperature, °C, if a probe is fitted.
    pub temp_c: Option<f32>,
}

/// Why a frame was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not one of ours: wrong length or magic bytes.
    NotOurs,
    /// A protocol version this firmware doesn't understand.
    Version(u8),
}

const MAGIC: [u8; 2] = *b"CB";
const VERSION: u8 = 1;
const FLAG_STABLE: u8 = 1 << 0;
const FLAG_CALIBRATED: u8 = 1 << 1;
const NO_TEMP: i16 = i16::MIN;

impl PadMsg {
    /// Encoded size in bytes.
    pub const LEN: usize = 19;

    /// Little-endian frame: magic "CB", version, corner (0xFF = none), flags, seq, kg, raw,
    /// temperature in hundredths of a degree (`i16::MIN` when absent).
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0..2].copy_from_slice(&MAGIC);
        b[2] = VERSION;
        b[3] = corner_to_byte(self.corner);
        b[4] = if self.stable { FLAG_STABLE } else { 0 } | if self.calibrated { FLAG_CALIBRATED } else { 0 };
        b[5..9].copy_from_slice(&self.seq.to_le_bytes());
        b[9..13].copy_from_slice(&self.kg.to_le_bytes());
        b[13..17].copy_from_slice(&self.raw.to_le_bytes());
        let temp = match self.temp_c {
            Some(t) if t.is_finite() => (t * 100.0).clamp(-32767.0, 32767.0) as i16,
            _ => NO_TEMP,
        };
        b[17..19].copy_from_slice(&temp.to_le_bytes());
        b
    }

    /// Parses a frame produced by [`PadMsg::encode`].
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let b: &[u8; Self::LEN] = b.try_into().map_err(|_| DecodeError::NotOurs)?;
        if b[0..2] != MAGIC {
            return Err(DecodeError::NotOurs);
        }
        if b[2] != VERSION {
            return Err(DecodeError::Version(b[2]));
        }
        let le4 = |at: usize| [b[at], b[at + 1], b[at + 2], b[at + 3]];
        let temp = i16::from_le_bytes([b[17], b[18]]);
        Ok(Self {
            corner: corner_from_byte(b[3]),
            stable: b[4] & FLAG_STABLE != 0,
            calibrated: b[4] & FLAG_CALIBRATED != 0,
            seq: u32::from_le_bytes(le4(5)),
            kg: f32::from_le_bytes(le4(9)),
            raw: i32::from_le_bytes(le4(13)),
            temp_c: (temp != NO_TEMP).then(|| f32::from(temp) / 100.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PadMsg {
        PadMsg {
            corner: Some(Corner::LR),
            seq: 0xDEAD_BEEF,
            kg: 123.456,
            raw: -8_388_608,
            stable: true,
            calibrated: false,
            temp_c: Some(-12.34),
        }
    }

    #[test]
    fn round_trip() {
        let m = sample();
        let d = PadMsg::decode(&m.encode()).unwrap();
        assert_eq!(PadMsg { temp_c: None, ..d }, PadMsg { temp_c: None, ..m });
        assert!((d.temp_c.unwrap() + 12.34).abs() < 0.006);
    }

    #[test]
    fn missing_or_invalid_temperature() {
        for temp_c in [None, Some(f32::NAN)] {
            let d = PadMsg::decode(&PadMsg { temp_c, ..sample() }.encode()).unwrap();
            assert_eq!(d.temp_c, None);
        }
        let d = PadMsg::decode(&PadMsg { temp_c: Some(1e9), ..sample() }.encode()).unwrap();
        assert_eq!(d.temp_c, Some(327.67));
    }

    #[test]
    fn layout_is_stable() {
        let b = sample().encode();
        assert_eq!(&b[0..5], &[b'C', b'B', 1, 2, 0b01]);
        assert_eq!(&b[5..9], &[0xEF, 0xBE, 0xAD, 0xDE]);
    }

    #[test]
    fn rejects_foreign_frames() {
        let mut b = sample().encode();
        assert_eq!(PadMsg::decode(&b[..18]), Err(DecodeError::NotOurs));
        b[2] = 9;
        assert_eq!(PadMsg::decode(&b), Err(DecodeError::Version(9)));
        b[0] = b'X';
        assert_eq!(PadMsg::decode(&b), Err(DecodeError::NotOurs));
    }
}
