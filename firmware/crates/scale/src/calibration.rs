//! Multi-point linear calibration from raw counts to kilograms.
//!
//! Each point pairs a filtered count with the known weight on the pad. A
//! least-squares line through all points gives counts → kg; the residuals
//! show how far the pad is from linear, which is one of the bench tests.

use crate::math;
use crate::stats::{Line, fit_line};

/// Maximum number of calibration points stored per pad.
pub const MAX_POINTS: usize = 16;

/// Bytes needed to store a full [`Calibration`] with [`Calibration::encode`].
pub const ENCODED_MAX_LEN: usize = 2 + MAX_POINTS * 8;

const ENCODING_VERSION: u8 = 1;

/// One calibration point: a filtered reading and the known weight that produced it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CalPoint {
    /// Filtered HX711 counts.
    pub counts: i32,
    /// Known weight on the pad, kg.
    pub kg: f32,
}

/// A fitted conversion from counts to kg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// kg per count.
    pub slope: f64,
    /// kg at zero counts.
    pub intercept: f64,
    /// Largest absolute residual over the calibration points, kg.
    pub max_residual_kg: f32,
}

impl Fit {
    /// Passes counts through unchanged, so an uncalibrated pad reports counts as "kg".
    pub const IDENTITY: Fit = Fit { slope: 1.0, intercept: 0.0, max_residual_kg: 0.0 };

    /// Weight for a (filtered) count value.
    pub fn kg(&self, counts: f64) -> f64 {
        self.slope * counts + self.intercept
    }
}

/// Why a calibration operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalError {
    /// Already holding [`MAX_POINTS`] points.
    Full,
    /// The weight is not a finite number.
    InvalidKg,
    /// Fewer than two points.
    TooFewPoints,
    /// All points have the same count, so no slope can be fitted.
    Degenerate,
    /// Stored bytes are truncated, from an unknown version, or inconsistent.
    BadEncoding,
}

/// A pad's calibration points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Calibration {
    points: [CalPoint; MAX_POINTS],
    len: usize,
}

impl Calibration {
    /// No points.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a point. Repeated weights are fine and improve the fit.
    pub fn add(&mut self, p: CalPoint) -> Result<(), CalError> {
        if !p.kg.is_finite() {
            return Err(CalError::InvalidKg);
        }
        if self.len == MAX_POINTS {
            return Err(CalError::Full);
        }
        self.points[self.len] = p;
        self.len += 1;
        Ok(())
    }

    /// Removes the most recent point, if any.
    pub fn remove_last(&mut self) -> Option<CalPoint> {
        self.len = self.len.checked_sub(1)?;
        Some(self.points[self.len])
    }

    /// Removes all points.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// The stored points, oldest first.
    pub fn points(&self) -> &[CalPoint] {
        &self.points[..self.len]
    }

    /// Least-squares fit through all points.
    pub fn fit(&self) -> Result<Fit, CalError> {
        if self.len < 2 {
            return Err(CalError::TooFewPoints);
        }
        let pts = self.points().iter().map(|p| (f64::from(p.counts), f64::from(p.kg)));
        let Line { slope, intercept } = fit_line(pts).ok_or(CalError::Degenerate)?;
        let mut fit = Fit { slope, intercept, max_residual_kg: 0.0 };
        fit.max_residual_kg = self.residuals(&fit).map(|(_, r)| math::abs(f64::from(r))).fold(0.0, f64::max) as f32;
        Ok(fit)
    }

    /// Each point with its residual: fitted kg minus known kg.
    pub fn residuals<'a>(&'a self, fit: &'a Fit) -> impl Iterator<Item = (CalPoint, f32)> + 'a {
        self.points().iter().map(|p| (*p, (fit.kg(f64::from(p.counts)) - f64::from(p.kg)) as f32))
    }

    /// Serializes the points into `buf` for flash storage, returning the bytes used.
    ///
    /// Layout: version, point count, then per point `counts: i32` and `kg: f32`,
    /// little-endian. The fit is recomputed on load rather than stored.
    pub fn encode(&self, buf: &mut [u8; ENCODED_MAX_LEN]) -> usize {
        buf[0] = ENCODING_VERSION;
        buf[1] = self.len as u8;
        for (i, p) in self.points().iter().enumerate() {
            let at = 2 + i * 8;
            buf[at..at + 4].copy_from_slice(&p.counts.to_le_bytes());
            buf[at + 4..at + 8].copy_from_slice(&p.kg.to_le_bytes());
        }
        2 + self.len * 8
    }

    /// Restores points written by [`Calibration::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self, CalError> {
        let [version, len, rest @ ..] = bytes else {
            return Err(CalError::BadEncoding);
        };
        let len = usize::from(*len);
        if *version != ENCODING_VERSION || len > MAX_POINTS || rest.len() != len * 8 {
            return Err(CalError::BadEncoding);
        }
        let mut cal = Self::new();
        for chunk in rest.chunks_exact(8) {
            let counts = i32::from_le_bytes(chunk[0..4].try_into().unwrap());
            let kg = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
            cal.add(CalPoint { counts, kg }).map_err(|_| CalError::BadEncoding)?;
        }
        Ok(cal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal_from(points: &[(i32, f32)]) -> Calibration {
        let mut cal = Calibration::new();
        for &(counts, kg) in points {
            cal.add(CalPoint { counts, kg }).unwrap();
        }
        cal
    }

    #[test]
    fn two_point_fit_is_exact() {
        // Typical bench pad: zero offset ~ -120k counts, ~21k counts/kg.
        let cal = cal_from(&[(-120_000, 0.0), (300_000, 20.0)]);
        let fit = cal.fit().unwrap();
        assert!((fit.kg(-120_000.0)).abs() < 1e-9);
        assert!((fit.kg(300_000.0) - 20.0).abs() < 1e-9);
        assert!((fit.kg(90_000.0) - 10.0).abs() < 1e-9);
        assert!(fit.max_residual_kg < 1e-6);
    }

    #[test]
    fn residuals_reveal_nonlinearity() {
        // Middle point reads 0.3 kg high relative to the ends.
        let cal = cal_from(&[(0, 0.0), (10_000, 9.7), (20_000, 20.0)]);
        let fit = cal.fit().unwrap();
        let res: Vec<f32> = cal.residuals(&fit).map(|(_, r)| r).collect();
        assert_eq!(res.len(), 3);
        // Residuals of a least-squares fit sum to zero.
        assert!(res.iter().sum::<f32>().abs() < 1e-5);
        assert!(res[1] > 0.15);
        assert!((fit.max_residual_kg - res[1].abs()).abs() < 1e-6);
    }

    #[test]
    fn repeated_weights_are_averaged() {
        let cal = cal_from(&[(0, 0.0), (0, 0.0), (1000, 10.0), (1010, 10.0)]);
        let fit = cal.fit().unwrap();
        assert!((fit.kg(1005.0) - 10.0).abs() < 0.01);
    }

    #[test]
    fn fit_errors() {
        assert_eq!(Calibration::new().fit(), Err(CalError::TooFewPoints));
        assert_eq!(cal_from(&[(5, 0.0)]).fit(), Err(CalError::TooFewPoints));
        assert_eq!(cal_from(&[(5, 0.0), (5, 10.0)]).fit(), Err(CalError::Degenerate));
    }

    #[test]
    fn add_rejects_bad_input() {
        let mut cal = Calibration::new();
        assert_eq!(cal.add(CalPoint { counts: 0, kg: f32::NAN }), Err(CalError::InvalidKg));
        for i in 0..MAX_POINTS as i32 {
            cal.add(CalPoint { counts: i, kg: 0.0 }).unwrap();
        }
        assert_eq!(cal.add(CalPoint { counts: 99, kg: 0.0 }), Err(CalError::Full));
    }

    #[test]
    fn remove_last_and_clear() {
        let mut cal = cal_from(&[(1, 1.0), (2, 2.0)]);
        assert_eq!(cal.remove_last(), Some(CalPoint { counts: 2, kg: 2.0 }));
        assert_eq!(cal.points().len(), 1);
        cal.clear();
        assert_eq!(cal.remove_last(), None);
    }

    #[test]
    fn encode_decode_round_trip() {
        let cal = cal_from(&[(-120_000, 0.0), (300_000, 20.0), (i32::MIN, -1.5)]);
        let mut buf = [0u8; ENCODED_MAX_LEN];
        let n = cal.encode(&mut buf);
        assert_eq!(n, 2 + 3 * 8);
        assert_eq!(Calibration::decode(&buf[..n]), Ok(cal));
    }

    #[test]
    fn decode_round_trip_empty_and_full() {
        let mut buf = [0u8; ENCODED_MAX_LEN];
        let empty = Calibration::new();
        let n = empty.encode(&mut buf);
        assert_eq!(Calibration::decode(&buf[..n]), Ok(empty));

        let full = cal_from(&[(7, 7.0); MAX_POINTS]);
        let n = full.encode(&mut buf);
        assert_eq!(n, ENCODED_MAX_LEN);
        assert_eq!(Calibration::decode(&buf[..n]), Ok(full));
    }

    #[test]
    fn decode_rejects_bad_bytes() {
        assert_eq!(Calibration::decode(&[]), Err(CalError::BadEncoding));
        assert_eq!(Calibration::decode(&[1]), Err(CalError::BadEncoding));
        assert_eq!(Calibration::decode(&[2, 0]), Err(CalError::BadEncoding));
        assert_eq!(Calibration::decode(&[1, 1, 0, 0, 0]), Err(CalError::BadEncoding));
        assert_eq!(Calibration::decode(&[1, 17]), Err(CalError::BadEncoding));
        let nan = f32::NAN.to_le_bytes();
        let bytes = [1, 1, 0, 0, 0, 0, nan[0], nan[1], nan[2], nan[3]];
        assert_eq!(Calibration::decode(&bytes), Err(CalError::BadEncoding));
    }
}
