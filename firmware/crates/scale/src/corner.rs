//! Corner-weight math.

/// Weight distribution of a car across four pads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerResult {
    /// Sum of all four corners, kg.
    pub total: f32,
    /// Front axle share of the total, %.
    pub front_pct: f32,
    /// Left side share of the total, %.
    pub left_pct: f32,
    /// Cross weight, (LF + RR) / total, %. 50 % is the usual target.
    pub cross_pct: f32,
}

/// Distribution from the four corner weights: left-front, right-front, left-rear, right-rear.
///
/// Returns `None` unless the total is positive and every input is finite.
pub fn corners(lf: f32, rf: f32, lr: f32, rr: f32) -> Option<CornerResult> {
    let total = lf + rf + lr + rr;
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let pct = |part: f32| part / total * 100.0;
    Some(CornerResult { total, front_pct: pct(lf + rf), left_pct: pct(lf + lr), cross_pct: pct(lf + rr) })
}

/// Share of the combined weight on pad `b`, %. Used by the two-pad bench test.
///
/// Returns `None` unless the combined weight is positive and finite.
pub fn split(a: f32, b: f32) -> Option<f32> {
    let total = a + b;
    (total.is_finite() && total > 0.0).then(|| b / total * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn balanced_car() {
        let r = corners(300.0, 300.0, 250.0, 250.0).unwrap();
        assert!(close(r.total, 1100.0));
        assert!(close(r.front_pct, 54.545));
        assert!(close(r.left_pct, 50.0));
        assert!(close(r.cross_pct, 50.0));
    }

    #[test]
    fn cross_weight_uses_lf_and_rr() {
        // Heavy left-front and right-rear diagonal.
        let r = corners(320.0, 280.0, 230.0, 270.0).unwrap();
        assert!(close(r.cross_pct, 590.0 / 1100.0 * 100.0));
        assert!(close(r.left_pct, 550.0 / 1100.0 * 100.0));
    }

    #[test]
    fn rejects_empty_or_invalid() {
        assert_eq!(corners(0.0, 0.0, 0.0, 0.0), None);
        assert_eq!(corners(-1.0, 0.0, 0.0, 0.0), None);
        assert_eq!(corners(f32::NAN, 1.0, 1.0, 1.0), None);
    }

    #[test]
    fn two_pad_split_follows_the_lever_rule() {
        // 20 kg at 30 % of the span from pad A: A carries 70 %, B carries 30 %.
        assert!(close(split(14.0, 6.0).unwrap(), 30.0));
        assert_eq!(split(0.0, 0.0), None);
        assert_eq!(split(f32::INFINITY, 1.0), None);
    }
}
