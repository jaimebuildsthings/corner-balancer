//! Float helpers that `core` lacks on `no_std` targets.

/// Absolute value.
pub(crate) fn abs(x: f64) -> f64 {
    if x < 0.0 { -x } else { x }
}

/// Square root by Newton's method. Returns NaN for negative or NaN input.
pub(crate) fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    // Halving the exponent bits gives a guess within a factor of ~2,
    // so a handful of iterations reaches full f64 precision.
    let mut y = f64::from_bits((x.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..8 {
        y = 0.5 * (y + x / y);
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_matches_std() {
        for &x in &[1e-12, 0.25, 1.0, 2.0, 50.0, 12345.678, 8.4e6, 1e18] {
            let rel = (sqrt(x) - x.sqrt()).abs() / x.sqrt();
            assert!(rel < 1e-15, "sqrt({x}) off by {rel}");
        }
    }

    #[test]
    fn sqrt_edge_cases() {
        assert_eq!(sqrt(0.0), 0.0);
        assert!(sqrt(-1.0).is_nan());
        assert!(sqrt(f64::NAN).is_nan());
        assert_eq!(sqrt(f64::INFINITY), f64::INFINITY);
    }
}
