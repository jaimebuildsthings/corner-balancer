//! Running statistics and least-squares line fitting.

use crate::math;

/// Running count, mean, standard deviation and range (Welford's algorithm).
#[derive(Clone, Copy, Debug)]
pub struct Running {
    n: u32,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
}

impl Default for Running {
    fn default() -> Self {
        Self::new()
    }
}

impl Running {
    /// An empty accumulator.
    pub const fn new() -> Self {
        Self { n: 0, mean: 0.0, m2: 0.0, min: f64::INFINITY, max: f64::NEG_INFINITY }
    }

    /// Adds one value. Non-finite values are ignored.
    pub fn push(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        self.n += 1;
        let delta = x - self.mean;
        self.mean += delta / f64::from(self.n);
        self.m2 += delta * (x - self.mean);
        self.min = self.min.min(x);
        self.max = self.max.max(x);
    }

    /// Number of values added.
    pub fn count(&self) -> u32 {
        self.n
    }

    /// Mean, once at least one value has been added.
    pub fn mean(&self) -> Option<f64> {
        (self.n > 0).then_some(self.mean)
    }

    /// Sample standard deviation (n − 1), once at least two values have been added.
    pub fn std_dev(&self) -> Option<f64> {
        (self.n > 1).then(|| math::sqrt(self.m2 / f64::from(self.n - 1)))
    }

    /// Smallest value.
    pub fn min(&self) -> Option<f64> {
        (self.n > 0).then_some(self.min)
    }

    /// Largest value.
    pub fn max(&self) -> Option<f64> {
        (self.n > 0).then_some(self.max)
    }

    /// Largest minus smallest value.
    pub fn spread(&self) -> Option<f64> {
        (self.n > 0).then_some(self.max - self.min)
    }
}

/// A straight line `y = slope · x + intercept`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Change in y per unit x.
    pub slope: f64,
    /// y at x = 0.
    pub intercept: f64,
}

impl Line {
    /// y at `x`.
    pub fn at(&self, x: f64) -> f64 {
        self.slope * x + self.intercept
    }
}

/// Ordinary least-squares fit of y on x.
///
/// Returns `None` with fewer than two points or when every x is the same.
/// Sums are taken about the mean so large raw counts don't lose precision.
pub fn fit_line<I>(points: I) -> Option<Line>
where
    I: Iterator<Item = (f64, f64)> + Clone,
{
    let mut n = 0u32;
    let (mut sx, mut sy) = (0.0, 0.0);
    for (x, y) in points.clone() {
        n += 1;
        sx += x;
        sy += y;
    }
    if n < 2 {
        return None;
    }
    let (mx, my) = (sx / f64::from(n), sy / f64::from(n));
    let (mut sxx, mut sxy) = (0.0, 0.0);
    for (x, y) in points {
        sxx += (x - mx) * (x - mx);
        sxy += (x - mx) * (y - my);
    }
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    Some(Line { slope, intercept: my - slope * mx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_matches_direct_computation() {
        let xs = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let mut r = Running::new();
        xs.iter().for_each(|&x| r.push(x));
        assert_eq!(r.count(), 8);
        assert_eq!(r.mean(), Some(5.0));
        // Sample variance: sum of squared deviations 32, divided by 7.
        assert!((r.std_dev().unwrap() - (32.0f64 / 7.0).sqrt()).abs() < 1e-12);
        assert_eq!(r.min(), Some(2.0));
        assert_eq!(r.max(), Some(9.0));
        assert_eq!(r.spread(), Some(7.0));
    }

    #[test]
    fn running_empty_and_single() {
        let mut r = Running::new();
        assert_eq!(r.mean(), None);
        assert_eq!(r.spread(), None);
        r.push(3.0);
        assert_eq!(r.mean(), Some(3.0));
        assert_eq!(r.std_dev(), None);
        assert_eq!(r.spread(), Some(0.0));
    }

    #[test]
    fn running_ignores_non_finite() {
        let mut r = Running::new();
        r.push(f64::NAN);
        r.push(f64::INFINITY);
        r.push(1.0);
        assert_eq!(r.count(), 1);
    }

    #[test]
    fn running_is_stable_with_large_offset() {
        // Raw HX711 counts sit near 8 million with a few counts of noise.
        let mut r = Running::new();
        for i in 0..1000 {
            r.push(8_000_000.0 + f64::from(i % 2));
        }
        assert!((r.std_dev().unwrap() - 0.50025).abs() < 1e-4);
    }

    #[test]
    fn fit_exact_line() {
        let pts = [(0.0, 1.0), (1.0, 3.0), (2.0, 5.0), (3.0, 7.0)];
        let line = fit_line(pts.iter().copied()).unwrap();
        assert!((line.slope - 2.0).abs() < 1e-12);
        assert!((line.intercept - 1.0).abs() < 1e-12);
        assert!((line.at(10.0) - 21.0).abs() < 1e-9);
    }

    #[test]
    fn fit_rejects_degenerate_input() {
        assert_eq!(fit_line([(1.0, 1.0)].iter().copied()), None);
        assert_eq!(fit_line([(2.0, 1.0), (2.0, 5.0)].iter().copied()), None);
        assert_eq!(fit_line(core::iter::empty()), None);
    }

    #[test]
    fn fit_precise_with_large_x() {
        // Counts near 2^23 with a slope of ~1e-5 kg/count.
        let slope = 2.5e-5;
        let pts: Vec<(f64, f64)> =
            (0..6).map(|i| 8_000_000.0 + 400_000.0 * f64::from(i)).map(|x| (x, slope * x - 200.0)).collect();
        let line = fit_line(pts.iter().copied()).unwrap();
        assert!((line.slope - slope).abs() / slope < 1e-12);
        assert!((line.intercept + 200.0).abs() < 1e-6);
    }
}
