//! CSV rows streamed over USB serial.
//!
//! Data rows start with a digit; everything else on the serial port (console
//! replies, log lines) does not, so a capture can be filtered with
//! `grep -E '^(t_ms|[0-9])'`.

use core::fmt::{self, Write};

/// Header line, without the trailing newline.
pub const HEADER: &str = "t_ms,ch,raw,filtered,kg,stable,temp_c";

/// One CSV row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    /// Milliseconds since boot.
    pub t_ms: u64,
    /// Load cell channel.
    pub ch: u8,
    /// Raw HX711 count.
    pub raw: i32,
    /// Filtered counts.
    pub filtered: f64,
    /// Weight after calibration and tare, kg.
    pub kg: f32,
    /// The reading has settled.
    pub stable: bool,
    /// Temperature, °C, if a probe is fitted.
    pub temp_c: Option<f32>,
}

impl fmt::Display for Row {
    /// Formats the row without a trailing newline. Empty `temp_c` when absent.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{},{},{},{:.1},{:.4},{},",
            self.t_ms,
            self.ch,
            self.raw,
            self.filtered,
            self.kg,
            u8::from(self.stable)
        )?;
        if let Some(t) = self.temp_c {
            write!(f, "{t:.2}")?;
        }
        Ok(())
    }
}

/// Writes `row` followed by `\r\n`.
pub fn write_row<W: Write>(w: &mut W, row: &Row) -> fmt::Result {
    write!(w, "{row}\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rows() {
        let row = Row {
            t_ms: 12_345,
            ch: 0,
            raw: -120_001,
            filtered: -119_998.25,
            kg: 20.00049,
            stable: true,
            temp_c: Some(21.5),
        };
        assert_eq!(row.to_string(), "12345,0,-120001,-119998.2,20.0005,1,21.50");
        let row = Row { temp_c: None, stable: false, ..row };
        let mut s = String::new();
        write_row(&mut s, &row).unwrap();
        assert_eq!(s, "12345,0,-120001,-119998.2,20.0005,0,\r\n");
    }

    #[test]
    fn row_matches_header_columns() {
        let row = Row { t_ms: 0, ch: 0, raw: 0, filtered: 0.0, kg: 0.0, stable: false, temp_c: None };
        assert_eq!(row.to_string().split(',').count(), HEADER.split(',').count());
    }
}
