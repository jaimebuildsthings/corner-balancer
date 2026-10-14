//! The scales protocol: one JSON object per line, for apps such as a web page
//! reading the scales over Web Serial (and later Web Bluetooth).
//! The format is documented in `docs/scales-protocol.md`; keep the two in step.

use core::fmt::{self, Write};

/// Protocol version, the `v` field. Bump on incompatible changes.
pub const VERSION: u8 = 1;

/// One pad's latest reading as seen by the pad sending the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadReading {
    /// Pad ID, 0–3: LF, RF, LR, RR.
    pub id: u8,
    /// Weight after calibration and tare, kg (counts if not calibrated).
    pub kg: f32,
    /// The reading has settled.
    pub stable: bool,
    /// The pad is calibrated, so `kg` really is kilograms.
    pub calibrated: bool,
    /// Time since this reading arrived, ms (0 for the sending pad itself).
    pub age_ms: u64,
}

/// Writes one frame, without a line ending:
/// `{"v":1,"type":"weights","t_ms":…,"pads":[{"id":0,"kg":14.001,"stable":true,"calibrated":true,"age_ms":0},…]}`
pub fn write_weights<W: Write>(w: &mut W, t_ms: u64, pads: &[PadReading]) -> fmt::Result {
    write!(w, r#"{{"v":{VERSION},"type":"weights","t_ms":{t_ms},"pads":["#)?;
    for (i, p) in pads.iter().enumerate() {
        if i > 0 {
            w.write_char(',')?;
        }
        write!(w, r#"{{"id":{},"kg":"#, p.id)?;
        // JSON has no NaN or infinity.
        if p.kg.is_finite() {
            write!(w, "{:.3}", p.kg)?;
        } else {
            w.write_str("null")?;
        }
        write!(w, r#","stable":{},"calibrated":{},"age_ms":{}}}"#, p.stable, p.calibrated, p.age_ms)?;
    }
    w.write_str("]}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_frame() {
        let pads = [
            PadReading { id: 0, kg: 14.0012, stable: true, calibrated: true, age_ms: 0 },
            PadReading { id: 1, kg: -0.25, stable: false, calibrated: false, age_ms: 80 },
        ];
        let mut s = String::new();
        write_weights(&mut s, 1234, &pads).unwrap();
        assert_eq!(
            s,
            r#"{"v":1,"type":"weights","t_ms":1234,"pads":[{"id":0,"kg":14.001,"stable":true,"calibrated":true,"age_ms":0},{"id":1,"kg":-0.250,"stable":false,"calibrated":false,"age_ms":80}]}"#
        );
    }

    #[test]
    fn empty_and_non_finite() {
        let mut s = String::new();
        write_weights(&mut s, 0, &[]).unwrap();
        assert_eq!(s, r#"{"v":1,"type":"weights","t_ms":0,"pads":[]}"#);
        let mut s = String::new();
        let p = PadReading { id: 2, kg: f32::NAN, stable: false, calibrated: true, age_ms: 5 };
        write_weights(&mut s, 0, &[p]).unwrap();
        assert!(s.contains(r#""kg":null"#), "{s}");
    }

    #[test]
    fn four_pads_fit_the_firmware_line_buffer() {
        let p = PadReading { id: 3, kg: -1234.567, stable: false, calibrated: false, age_ms: 99_999 };
        let mut s = String::new();
        write_weights(&mut s, u64::from(u32::MAX), &[p; 4]).unwrap();
        assert!(s.len() < 480, "{} bytes", s.len());
    }
}
