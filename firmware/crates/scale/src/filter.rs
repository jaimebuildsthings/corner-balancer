//! The per-channel filter: median of 5, exponential moving average, tare, stability.

use crate::calibration::Fit;

/// Longest stability window, in samples.
pub const MAX_STABLE_SAMPLES: usize = 64;

const MEDIAN_LEN: usize = 5;

/// Filter tuning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterConfig {
    /// Weight of each new median in the moving average, in (0, 1].
    /// 1 disables averaging; smaller is smoother and slower.
    pub ema_alpha: f32,
    /// A reading is stable when the last `stable_samples` readings span at most this many kg.
    pub stable_band_kg: f32,
    /// Stability window length, 1..=[`MAX_STABLE_SAMPLES`].
    pub stable_samples: u16,
}

impl Default for FilterConfig {
    /// Tuned for the HX711 at 10 samples/s: settles in about a second.
    fn default() -> Self {
        Self { ema_alpha: 0.3, stable_band_kg: 0.05, stable_samples: 10 }
    }
}

/// One filtered reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The raw count that produced this reading.
    pub raw: i32,
    /// Counts after the median and moving average.
    pub filtered: f64,
    /// Weight after calibration and tare, kg.
    pub kg: f32,
    /// True when recent readings have settled within the stability band.
    pub stable: bool,
}

/// Turns raw counts from one load cell channel into filtered kilograms.
#[derive(Clone, Debug)]
pub struct Scale {
    cfg: FilterConfig,
    fit: Fit,
    tare_counts: Option<f64>,
    median: [i32; MEDIAN_LEN],
    median_len: usize,
    median_pos: usize,
    ema: Option<f64>,
    // Untared kg, so taring doesn't disturb the stability window.
    recent: [f32; MAX_STABLE_SAMPLES],
    recent_len: usize,
    recent_pos: usize,
}

impl Scale {
    /// A scale with the given tuning and calibration, not tared.
    pub fn new(cfg: FilterConfig, fit: Fit) -> Self {
        let cfg = FilterConfig {
            ema_alpha: if cfg.ema_alpha > 0.0 && cfg.ema_alpha <= 1.0 { cfg.ema_alpha } else { 1.0 },
            stable_band_kg: cfg.stable_band_kg,
            stable_samples: cfg.stable_samples.clamp(1, MAX_STABLE_SAMPLES as u16),
        };
        Self {
            cfg,
            fit,
            tare_counts: None,
            median: [0; MEDIAN_LEN],
            median_len: 0,
            median_pos: 0,
            ema: None,
            recent: [0.0; MAX_STABLE_SAMPLES],
            recent_len: 0,
            recent_pos: 0,
        }
    }

    /// The active tuning, after clamping.
    pub fn config(&self) -> FilterConfig {
        self.cfg
    }

    /// The active calibration.
    pub fn fit(&self) -> Fit {
        self.fit
    }

    /// Feeds one raw count. Returns `None` until the median window has filled.
    pub fn push(&mut self, raw: i32) -> Option<Reading> {
        self.median[self.median_pos] = raw;
        self.median_pos = (self.median_pos + 1) % MEDIAN_LEN;
        self.median_len = (self.median_len + 1).min(MEDIAN_LEN);
        if self.median_len < MEDIAN_LEN {
            return None;
        }
        let mut sorted = self.median;
        sorted.sort_unstable();
        let median = f64::from(sorted[MEDIAN_LEN / 2]);

        let alpha = f64::from(self.cfg.ema_alpha);
        let filtered = match self.ema {
            Some(prev) => prev + alpha * (median - prev),
            None => median,
        };
        self.ema = Some(filtered);

        let gross = self.fit.kg(filtered) as f32;
        self.recent[self.recent_pos] = gross;
        self.recent_pos = (self.recent_pos + 1) % MAX_STABLE_SAMPLES;
        self.recent_len = (self.recent_len + 1).min(MAX_STABLE_SAMPLES);

        Some(Reading { raw, filtered, kg: self.net_kg(filtered), stable: self.is_stable() })
    }

    /// Zeroes the scale at the current filtered value. Returns false if there is no reading yet.
    pub fn tare(&mut self) -> bool {
        self.tare_counts = self.ema;
        self.ema.is_some()
    }

    /// Removes the tare, so readings are absolute per the calibration.
    pub fn clear_tare(&mut self) {
        self.tare_counts = None;
    }

    /// Replaces the calibration. The tare is kept, since it is stored in counts.
    pub fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
        self.recent_len = 0;
    }

    /// Discards filter history, e.g. after the amplifier was power-cycled.
    pub fn reset(&mut self) {
        self.median_len = 0;
        self.median_pos = 0;
        self.ema = None;
        self.recent_len = 0;
    }

    fn net_kg(&self, filtered: f64) -> f32 {
        match self.tare_counts {
            Some(tare) => (self.fit.slope * (filtered - tare)) as f32,
            None => self.fit.kg(filtered) as f32,
        }
    }

    fn is_stable(&self) -> bool {
        let window = usize::from(self.cfg.stable_samples);
        if self.recent_len < window {
            return false;
        }
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..window {
            let v = self.recent[(self.recent_pos + MAX_STABLE_SAMPLES - 1 - i) % MAX_STABLE_SAMPLES];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo <= self.cfg.stable_band_kg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNFILTERED: FilterConfig = FilterConfig { ema_alpha: 1.0, stable_band_kg: 0.05, stable_samples: 3 };
    // 1000 counts per kg, zero at 50_000 counts.
    const FIT: Fit = Fit { slope: 0.001, intercept: -50.0, max_residual_kg: 0.0 };

    fn feed(s: &mut Scale, raws: &[i32]) -> Vec<Reading> {
        raws.iter().filter_map(|&r| s.push(r)).collect()
    }

    #[test]
    fn waits_for_median_window() {
        let mut s = Scale::new(UNFILTERED, FIT);
        assert!(feed(&mut s, &[1, 2, 3, 4]).is_empty());
        assert!(s.push(5).is_some());
    }

    #[test]
    fn median_rejects_single_spikes() {
        let mut s = Scale::new(UNFILTERED, Fit::IDENTITY);
        let out = feed(&mut s, &[100, 100, 9_000_000, 100, 100, -9_000_000, 100, 100]);
        assert!(out.iter().all(|r| r.filtered == 100.0), "{out:?}");
        assert_eq!(out[0].raw, 100);
    }

    #[test]
    fn median_passes_a_real_step() {
        let mut s = Scale::new(UNFILTERED, Fit::IDENTITY);
        let out = feed(&mut s, &[0, 0, 0, 0, 0, 500, 500, 500]);
        assert_eq!(out.last().unwrap().filtered, 500.0);
    }

    #[test]
    fn ema_converges_geometrically() {
        let cfg = FilterConfig { ema_alpha: 0.5, ..UNFILTERED };
        let mut s = Scale::new(cfg, Fit::IDENTITY);
        feed(&mut s, &[0; 5]);
        // After the median follows the step, each sample halves the remaining gap.
        let out = feed(&mut s, &[1000; 6]);
        let tail: Vec<f64> = out.iter().map(|r| r.filtered).skip(2).collect();
        assert_eq!(tail, vec![500.0, 750.0, 875.0, 937.5]);
    }

    #[test]
    fn invalid_alpha_disables_averaging() {
        for alpha in [0.0, -1.0, 1.5, f32::NAN] {
            let s = Scale::new(FilterConfig { ema_alpha: alpha, ..UNFILTERED }, FIT);
            assert_eq!(s.config().ema_alpha, 1.0);
        }
        let s = Scale::new(FilterConfig { stable_samples: 0, ..UNFILTERED }, FIT);
        assert_eq!(s.config().stable_samples, 1);
        let s = Scale::new(FilterConfig { stable_samples: 1000, ..UNFILTERED }, FIT);
        assert_eq!(usize::from(s.config().stable_samples), MAX_STABLE_SAMPLES);
    }

    #[test]
    fn applies_calibration() {
        let mut s = Scale::new(UNFILTERED, FIT);
        let r = feed(&mut s, &[70_000; 5]).pop().unwrap();
        assert!((r.kg - 20.0).abs() < 1e-4);
    }

    #[test]
    fn tare_zeroes_and_survives_recalibration() {
        let mut s = Scale::new(UNFILTERED, FIT);
        assert!(!s.tare(), "nothing to tare yet");
        feed(&mut s, &[52_000; 5]);
        assert!(s.tare());
        let r = feed(&mut s, &[72_000; 5]).pop().unwrap();
        assert!((r.kg - 20.0).abs() < 1e-4);

        // Doubling the slope doubles the net weight; the tare point stays put.
        s.set_fit(Fit { slope: 0.002, ..FIT });
        let r = s.push(72_000).unwrap();
        assert!((r.kg - 40.0).abs() < 1e-4);

        s.clear_tare();
        let r = s.push(72_000).unwrap();
        assert!((r.kg - (0.002 * 72_000.0 - 50.0) as f32).abs() < 1e-3);
    }

    #[test]
    fn stability_flag() {
        let mut s = Scale::new(UNFILTERED, FIT);
        let out = feed(&mut s, &[50_000, 50_000, 50_000, 50_000, 50_000, 50_010, 50_020]);
        // Window of 3 needs three readings first.
        assert_eq!(out.iter().map(|r| r.stable).collect::<Vec<_>>(), vec![false, false, true]);
        // A 0.5 kg step reaches the output after the median's two-sample delay,
        // breaks stability, and three settled readings restore it.
        let out = feed(&mut s, &[50_500; 6]);
        assert_eq!(out.iter().map(|r| r.stable).collect::<Vec<_>>(), vec![true, true, false, false, true, true]);
    }

    #[test]
    fn tare_does_not_break_stability() {
        let mut s = Scale::new(UNFILTERED, FIT);
        feed(&mut s, &[60_000; 8]);
        s.tare();
        assert!(s.push(60_000).unwrap().stable);
    }

    #[test]
    fn reset_restarts_the_filter() {
        let mut s = Scale::new(UNFILTERED, FIT);
        feed(&mut s, &[60_000; 8]);
        s.reset();
        assert!(feed(&mut s, &[60_000; 4]).is_empty());
        assert!(!s.push(60_000).unwrap().stable);
    }
}
