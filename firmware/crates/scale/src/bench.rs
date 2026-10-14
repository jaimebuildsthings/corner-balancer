//! The bench test protocol from `docs/test-protocol.md`, as small state machines.
//!
//! Each test is fed [`Reading`]s and reports a result when finished. The firmware
//! drives them from the serial console (and later the phone UI); pass targets
//! scale with the pad's full-scale rating so the same code serves phase 2.

use crate::calibration::Fit;
use crate::filter::Reading;
use crate::math;
use crate::stats::Running;

/// Pass targets for one pad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Targets {
    /// Zero noise: max standard deviation unloaded, kg.
    pub noise_sd_kg: f32,
    /// Repeatability: max spread over repeated placements of one weight, kg.
    pub repeat_spread_kg: f32,
    /// Linearity: max calibration residual, kg.
    pub linearity_kg: f32,
    /// Creep: max drift under a held load, kg.
    pub creep_kg: f32,
    /// Return to zero after unloading, kg.
    pub zero_return_kg: f32,
    /// Off-center: max spread as a fraction of the mean reading.
    pub off_center_frac: f32,
}

impl Targets {
    /// The plan's targets, which are 50 g / 200 g / 500 g / 200 g / 50 g at 200 kg,
    /// expressed as fractions of full scale.
    pub fn for_full_scale(full_scale_kg: f32) -> Self {
        Self {
            noise_sd_kg: 0.00025 * full_scale_kg,
            repeat_spread_kg: 0.001 * full_scale_kg,
            linearity_kg: 0.0025 * full_scale_kg,
            creep_kg: 0.001 * full_scale_kg,
            zero_return_kg: 0.00025 * full_scale_kg,
            off_center_frac: 0.005,
        }
    }

    /// Zero noise passes.
    pub fn noise_pass(&self, r: &NoiseResult) -> bool {
        r.std_dev_kg <= self.noise_sd_kg
    }

    /// Repeatability passes.
    pub fn repeat_pass(&self, r: &PlaceResult) -> bool {
        r.spread_kg <= self.repeat_spread_kg
    }

    /// Off-center loading passes.
    pub fn off_center_pass(&self, r: &PlaceResult) -> bool {
        r.spread_frac().is_some_and(|f| f <= self.off_center_frac)
    }

    /// Linearity passes for a calibration fit.
    pub fn linearity_pass(&self, fit: &Fit) -> bool {
        fit.max_residual_kg <= self.linearity_kg
    }

    /// Creep passes.
    pub fn creep_pass(&self, r: &CreepResult) -> bool {
        r.max_drift_kg <= self.creep_kg
    }

    /// Return to zero passes for a stable unloaded reading.
    pub fn zero_return_pass(&self, kg: f32) -> bool {
        math::abs(f64::from(kg)) <= f64::from(self.zero_return_kg)
    }
}

/// Zero noise: the spread of a fixed number of readings with the pad unloaded.
#[derive(Clone, Debug)]
pub struct NoiseTest {
    samples: u32,
    stats: Running,
}

/// Result of a [`NoiseTest`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseResult {
    /// Readings taken.
    pub samples: u32,
    /// Mean, kg.
    pub mean_kg: f32,
    /// Sample standard deviation, kg.
    pub std_dev_kg: f32,
}

impl NoiseTest {
    /// Collects `samples` readings (at least 2).
    pub fn new(samples: u32) -> Self {
        Self { samples: samples.max(2), stats: Running::new() }
    }

    /// Feeds a reading; returns the result on the last one.
    pub fn push(&mut self, r: &Reading) -> Option<NoiseResult> {
        self.stats.push(f64::from(r.kg));
        (self.stats.count() >= self.samples).then(|| NoiseResult {
            samples: self.stats.count(),
            mean_kg: self.stats.mean().unwrap_or(0.0) as f32,
            std_dev_kg: self.stats.std_dev().unwrap_or(0.0) as f32,
        })
    }
}

/// Repeatability and off-center: place a weight, let it settle, remove it, repeat.
///
/// A placement is recorded at the first stable reading above `threshold_kg`;
/// the next one is accepted only after a stable reading below it.
#[derive(Clone, Debug)]
pub struct PlaceRemoveTest {
    placements: u32,
    threshold_kg: f32,
    loaded: bool,
    stats: Running,
}

/// Progress of a [`PlaceRemoveTest`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaceEvent {
    /// A placement was recorded. `index` counts from 1.
    Recorded {
        /// Placement number, from 1.
        index: u32,
        /// The stable reading, kg.
        kg: f32,
    },
    /// The pad was unloaded; place the weight again.
    Unloaded,
    /// All placements are done.
    Done(PlaceResult),
}

/// Result of a [`PlaceRemoveTest`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceResult {
    /// Placements recorded.
    pub count: u32,
    /// Mean reading, kg.
    pub mean_kg: f32,
    /// Largest minus smallest reading, kg.
    pub spread_kg: f32,
}

impl PlaceResult {
    /// Spread as a fraction of the mean reading.
    pub fn spread_frac(&self) -> Option<f32> {
        (self.mean_kg > 0.0).then(|| self.spread_kg / self.mean_kg)
    }
}

impl PlaceRemoveTest {
    /// Records `placements` placements (at least 2) of a load heavier than `threshold_kg`.
    /// Tare the empty pad before starting.
    pub fn new(placements: u32, threshold_kg: f32) -> Self {
        Self { placements: placements.max(2), threshold_kg, loaded: false, stats: Running::new() }
    }

    /// Feeds a reading; returns an event when something changes.
    pub fn push(&mut self, r: &Reading) -> Option<PlaceEvent> {
        if !r.stable {
            return None;
        }
        if !self.loaded && r.kg > self.threshold_kg {
            self.loaded = true;
            self.stats.push(f64::from(r.kg));
            let index = self.stats.count();
            if index >= self.placements {
                return Some(PlaceEvent::Done(PlaceResult {
                    count: index,
                    mean_kg: self.stats.mean().unwrap_or(0.0) as f32,
                    spread_kg: self.stats.spread().unwrap_or(0.0) as f32,
                }));
            }
            return Some(PlaceEvent::Recorded { index, kg: r.kg });
        }
        if self.loaded && r.kg < self.threshold_kg {
            self.loaded = false;
            return Some(PlaceEvent::Unloaded);
        }
        None
    }
}

/// Creep: hold a load and track drift from the first stable reading.
#[derive(Clone, Debug)]
pub struct CreepTest {
    duration_ms: u64,
    start: Option<(u64, f32)>,
    max_drift_kg: f32,
}

/// Result of a [`CreepTest`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreepResult {
    /// First stable reading, kg.
    pub start_kg: f32,
    /// Last reading, kg.
    pub end_kg: f32,
    /// Largest absolute difference from `start_kg`, kg.
    pub max_drift_kg: f32,
}

impl CreepTest {
    /// Runs for `duration_ms` after the load first settles.
    pub fn new(duration_ms: u64) -> Self {
        Self { duration_ms, start: None, max_drift_kg: 0.0 }
    }

    /// Feeds a reading taken at `t_ms`; returns the result once the duration has elapsed.
    pub fn push(&mut self, t_ms: u64, r: &Reading) -> Option<CreepResult> {
        let Some((t0, kg0)) = self.start else {
            if r.stable {
                self.start = Some((t_ms, r.kg));
            }
            return None;
        };
        let drift = math::abs(f64::from(r.kg - kg0)) as f32;
        self.max_drift_kg = self.max_drift_kg.max(drift);
        (t_ms.saturating_sub(t0) >= self.duration_ms).then_some(CreepResult {
            start_kg: kg0,
            end_kg: r.kg,
            max_drift_kg: self.max_drift_kg,
        })
    }

    /// True once the starting reading has been taken.
    pub fn started(&self) -> bool {
        self.start.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(kg: f32, stable: bool) -> Reading {
        Reading { raw: 0, filtered: 0.0, kg, stable }
    }

    #[test]
    fn targets_match_the_plan_at_200_kg() {
        let t = Targets::for_full_scale(200.0);
        assert!((t.noise_sd_kg - 0.05).abs() < 1e-6);
        assert!((t.repeat_spread_kg - 0.2).abs() < 1e-6);
        assert!((t.linearity_kg - 0.5).abs() < 1e-6);
        assert!((t.creep_kg - 0.2).abs() < 1e-6);
        assert!((t.zero_return_kg - 0.05).abs() < 1e-6);
        assert!(t.zero_return_pass(-0.04));
        assert!(!t.zero_return_pass(0.06));
    }

    #[test]
    fn noise_test() {
        let mut t = NoiseTest::new(4);
        let kgs = [0.01, -0.01, 0.01, -0.01];
        let results: Vec<_> = kgs.iter().filter_map(|&k| t.push(&reading(k, true))).collect();
        assert_eq!(results.len(), 1);
        let r = results[0];
        assert_eq!(r.samples, 4);
        assert!(r.mean_kg.abs() < 1e-9);
        assert!((r.std_dev_kg - (0.0004f32 / 3.0).sqrt()).abs() < 1e-6);
        assert!(Targets::for_full_scale(200.0).noise_pass(&r));
    }

    #[test]
    fn place_remove_sequence() {
        let mut t = PlaceRemoveTest::new(3, 1.0);
        let script = [
            (0.0, true),   // empty
            (12.0, false), // settling
            (20.0, true),  // recorded #1
            (20.0, true),  // still loaded: ignored
            (0.0, true),   // unloaded
            (20.1, true),  // recorded #2
            (0.5, false),  // unstable: ignored
            (0.0, true),   // unloaded
            (19.95, true), // done
        ];
        let events: Vec<_> = script.iter().filter_map(|&(k, s)| t.push(&reading(k, s))).collect();
        assert_eq!(events[0], PlaceEvent::Recorded { index: 1, kg: 20.0 });
        assert_eq!(events[1], PlaceEvent::Unloaded);
        assert_eq!(events[2], PlaceEvent::Recorded { index: 2, kg: 20.1 });
        assert_eq!(events[3], PlaceEvent::Unloaded);
        let PlaceEvent::Done(r) = events[4] else { panic!("{events:?}") };
        assert_eq!(events.len(), 5);
        assert_eq!(r.count, 3);
        assert!((r.mean_kg - 20.016_666).abs() < 1e-4);
        assert!((r.spread_kg - 0.15).abs() < 1e-4);
        let targets = Targets::for_full_scale(200.0);
        assert!(targets.repeat_pass(&r));
        // 0.15 kg on ~20 kg is a 0.75 % spread: fails the 0.5 % off-center target.
        assert!(!targets.off_center_pass(&r));
    }

    #[test]
    fn creep_test() {
        let mut t = CreepTest::new(10_000);
        assert_eq!(t.push(0, &reading(50.0, false)), None);
        assert!(!t.started());
        assert_eq!(t.push(1_000, &reading(50.0, true)), None);
        assert!(t.started());
        assert_eq!(t.push(5_000, &reading(50.15, true)), None);
        let r = t.push(11_000, &reading(50.1, true)).unwrap();
        assert_eq!(r.start_kg, 50.0);
        assert_eq!(r.end_kg, 50.1);
        assert!((r.max_drift_kg - 0.15).abs() < 1e-4);
        assert!(Targets::for_full_scale(200.0).creep_pass(&r));
    }
}
