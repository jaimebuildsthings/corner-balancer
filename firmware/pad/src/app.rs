//! The pad's main loop: filters samples, runs console commands and bench tests,
//! and streams CSV.

use core::sync::atomic::Ordering;

use embassy_time::{Duration, with_timeout};
use protocol::command::HELP;
use protocol::csv;
use protocol::{Command, ParseError};
use scale::bench::{CreepTest, NoiseTest, PlaceEvent, PlaceRemoveTest, Targets};
use scale::{CalPoint, Calibration, FilterConfig, Fit, Reading, Scale};

use crate::calib_store::CalibStore;
use crate::console::LINES;
use crate::out::say;
use crate::sampler::{DROPPED, SAMPLES, Sample};
use crate::{MAX_CHANNELS, temperature};

/// Default full-scale rating for pass targets: the phase 1 bench pad.
const DEFAULT_FULL_SCALE_KG: f32 = 200.0;

struct Channel {
    scale: Scale,
    cal: Calibration,
    calibrated: bool,
    saturated: bool,
    last: Option<Reading>,
}

enum Test {
    Noise(NoiseTest),
    Place { test: PlaceRemoveTest, off_center: bool },
    Creep(CreepTest),
    Zero,
}

/// Pad state on core 0.
pub struct App {
    channels: [Option<Channel>; MAX_CHANNELS],
    selected: usize,
    full_scale_kg: f32,
    stream: bool,
    test: Option<Test>,
    pending_cal_kg: Option<f32>,
    pending_tare: bool,
    /// Readings on the selected channel since the last `cal add` or `test zero`.
    fresh: u32,
    store: Option<CalibStore>,
}

impl App {
    /// `active` channels get a scale, using the stored calibration where there is one.
    pub fn new(active: usize, mut store: Option<CalibStore>) -> Self {
        let stored = store.as_mut().and_then(|s| s.load().ok()).unwrap_or_default();
        let channels = core::array::from_fn(|i| {
            (i < active).then(|| {
                let cal = stored[i].clone();
                let fit = cal.fit().ok();
                Channel {
                    scale: Scale::new(FilterConfig::default(), fit.unwrap_or(Fit::IDENTITY)),
                    cal,
                    calibrated: fit.is_some(),
                    saturated: false,
                    last: None,
                }
            })
        });
        Self {
            channels,
            selected: 0,
            full_scale_kg: DEFAULT_FULL_SCALE_KG,
            stream: false,
            test: None,
            pending_cal_kg: None,
            pending_tare: false,
            fresh: 0,
            store,
        }
    }

    /// Runs forever: handles samples as they arrive and console lines in between.
    pub async fn run(mut self) -> ! {
        say!("corner-balancer pad {} ({}). Type `help`.", env!("CARGO_PKG_VERSION"), crate::CHIP);
        if self.store.is_none() {
            say!("warning: no `calib` partition; calibration can't be saved. Flash with partitions.csv.");
        }
        loop {
            if let Ok(sample) = with_timeout(Duration::from_millis(50), SAMPLES.receive()).await {
                self.on_sample(sample).await;
            }
            while let Ok(line) = LINES.try_receive() {
                self.on_line(line.as_str()).await;
            }
        }
    }

    async fn on_sample(&mut self, s: Sample) {
        let ch = usize::from(s.ch);
        let Some(chan) = self.channels.get_mut(ch).and_then(Option::as_mut) else { return };
        let saturated = hx711::is_saturated(s.raw);
        if saturated && !chan.saturated {
            say!("warning: ch{ch} saturated ({}): overload or a loose wire", s.raw);
        }
        chan.saturated = saturated;
        let Some(r) = chan.scale.push(s.raw) else { return };
        chan.last = Some(r);
        if self.stream {
            let row = csv::Row {
                t_ms: s.t_ms,
                ch: s.ch,
                raw: r.raw,
                filtered: r.filtered,
                kg: r.kg,
                stable: r.stable,
                temp_c: temperature::latest(),
            };
            say!("{row}");
        }
        if ch == self.selected {
            self.on_selected_reading(s.t_ms, r).await;
        }
    }

    async fn on_selected_reading(&mut self, t_ms: u64, r: Reading) {
        // A reading only counts as settled once its whole stability window
        // (plus the median's two-sample delay) came after the command, so a
        // load placed just before typing can't be captured at its old value.
        self.fresh = self.fresh.saturating_add(1);
        let settled = r.stable && self.fresh > u32::from(self.selected_mut().scale.config().stable_samples) + 2;
        if self.pending_tare && settled {
            self.pending_tare = false;
            self.selected_mut().scale.tare();
            say!("tared at {:.0} counts", r.filtered);
        }
        if let Some(kg) = self.pending_cal_kg
            && settled
        {
            self.pending_cal_kg = None;
            let counts = round(r.filtered);
            let chan = self.selected_mut();
            match chan.cal.add(CalPoint { counts, kg }) {
                Ok(()) => say!("cal: point {}: {counts} counts = {kg} kg", chan.cal.points().len()),
                Err(e) => say!("cal: can't add point: {e:?}"),
            }
        }

        let targets = Targets::for_full_scale(self.full_scale_kg);
        let Some(test) = self.test.as_mut() else { return };
        match test {
            Test::Noise(t) => {
                if let Some(res) = t.push(&r) {
                    self.test = None;
                    say!(
                        "noise: {} readings, mean {:.4} kg, std dev {:.1} g (target <= {:.1} g): {}",
                        res.samples,
                        res.mean_kg,
                        res.std_dev_kg * 1000.0,
                        targets.noise_sd_kg * 1000.0,
                        verdict(targets.noise_pass(&res))
                    );
                }
            }
            Test::Place { test, off_center } => {
                let off_center = *off_center;
                match test.push(&r) {
                    Some(PlaceEvent::Recorded { index, kg }) => {
                        say!("placement {index}: {kg:.3} kg. Remove the weight.")
                    }
                    Some(PlaceEvent::Unloaded) => {
                        say!(
                            "{}",
                            if off_center { "Place the weight at the next position." } else { "Place the weight." }
                        )
                    }
                    Some(PlaceEvent::Done(res)) => {
                        self.test = None;
                        say!(
                            "{} placements, mean {:.3} kg, spread {:.1} g",
                            res.count,
                            res.mean_kg,
                            res.spread_kg * 1000.0
                        );
                        if off_center {
                            say!(
                                "off-center: spread {:.2} % (target <= {:.2} %): {}",
                                res.spread_frac().unwrap_or(f32::NAN) * 100.0,
                                targets.off_center_frac * 100.0,
                                verdict(targets.off_center_pass(&res))
                            );
                        } else {
                            say!(
                                "repeatability: spread {:.1} g (target <= {:.1} g): {}",
                                res.spread_kg * 1000.0,
                                targets.repeat_spread_kg * 1000.0,
                                verdict(targets.repeat_pass(&res))
                            );
                        }
                    }
                    None => {}
                }
            }
            Test::Creep(t) => {
                let was_started = t.started();
                let done = t.push(t_ms, &r);
                if !was_started && t.started() {
                    say!("creep: started at {:.3} kg; keep the load still.", r.kg);
                }
                if let Some(res) = done {
                    self.test = None;
                    say!(
                        "creep: {:.3} -> {:.3} kg, max drift {:.1} g (target <= {:.1} g): {}",
                        res.start_kg,
                        res.end_kg,
                        res.max_drift_kg * 1000.0,
                        targets.creep_kg * 1000.0,
                        verdict(targets.creep_pass(&res))
                    );
                    say!("Now unload the pad and run `test zero`.");
                }
            }
            Test::Zero => {
                if settled {
                    self.test = None;
                    say!(
                        "zero: {:.1} g (target within {:.1} g): {}",
                        r.kg * 1000.0,
                        targets.zero_return_kg * 1000.0,
                        verdict(targets.zero_return_pass(r.kg))
                    );
                }
            }
        }
    }

    async fn on_line(&mut self, line: &str) {
        let cmd = match Command::parse(line) {
            Ok(cmd) => cmd,
            Err(ParseError::Empty) => return,
            Err(e) => {
                say!("error: {e:?}. Type `help`.");
                return;
            }
        };
        match cmd {
            Command::Help => {
                for l in HELP.lines() {
                    say!("{l}");
                }
            }
            Command::Status => self.status().await,
            Command::Channel(ch) => {
                if self.channels.get(usize::from(ch)).is_some_and(Option::is_some) {
                    self.selected = usize::from(ch);
                    self.test = None;
                    self.pending_cal_kg = None;
                    self.pending_tare = false;
                    say!("channel {ch} selected");
                } else {
                    say!("error: no channel {ch}");
                }
            }
            Command::FullScale(kg) => {
                self.full_scale_kg = kg;
                say!("full scale {kg} kg");
            }
            Command::Stream(on) => {
                self.stream = on;
                crate::console::QUIET.store(on, Ordering::Relaxed);
                if on {
                    say!("{}", csv::HEADER);
                }
            }
            Command::Tare => {
                self.pending_tare = true;
                self.fresh = 0;
                say!("tare: waiting for a stable reading...");
            }
            Command::CalAdd(kg) => {
                self.pending_cal_kg = Some(kg);
                self.fresh = 0;
                say!("cal: waiting for a stable reading with {kg} kg on the pad...");
            }
            Command::CalUndo => match self.selected_mut().cal.remove_last() {
                Some(p) => say!("cal: removed {} counts = {} kg", p.counts, p.kg),
                None => say!("cal: no points"),
            },
            Command::CalClear => {
                self.selected_mut().cal.clear();
                say!("cal: points cleared (the active fit is unchanged until `cal fit`)");
            }
            Command::CalFit => self.cal_fit().await,
            Command::CalSave => {
                let cals: [Calibration; MAX_CHANNELS] =
                    core::array::from_fn(|i| self.channels[i].as_ref().map(|c| c.cal.clone()).unwrap_or_default());
                match self.store.as_mut().map(|s| s.save(&cals)) {
                    Some(Ok(())) => say!("cal: saved"),
                    Some(Err(e)) => say!("error: save failed: {e:?}"),
                    None => say!("error: no calib partition"),
                }
            }
            Command::CalShow => self.cal_show().await,
            Command::TestNoise(n) => {
                self.start(Test::Noise(NoiseTest::new(n)), "noise: leave the pad empty; starts once stable").await
            }
            Command::TestRepeat { placements, threshold_kg } => {
                let test = Test::Place { test: PlaceRemoveTest::new(placements, threshold_kg), off_center: false };
                self.start(test, "repeatability: tare the empty pad first, then place the weight").await
            }
            Command::TestOffCenter { placements, threshold_kg } => {
                let test = Test::Place { test: PlaceRemoveTest::new(placements, threshold_kg), off_center: true };
                self.start(test, "off-center: tare first; place the weight at the center, then each corner").await
            }
            Command::TestCreep(secs) => {
                let test = Test::Creep(CreepTest::new(u64::from(secs) * 1000));
                self.start(test, "creep: tare the empty pad first, then place a heavy load").await
            }
            Command::TestZero => {
                self.fresh = 0;
                self.start(Test::Zero, "zero: waiting for a stable reading").await
            }
            Command::Sim(kg) => {
                #[cfg(feature = "sim")]
                {
                    crate::sampler::SIM_KG.store(kg.to_bits(), Ordering::Relaxed);
                    say!("sim: load {kg} kg");
                }
                #[cfg(not(feature = "sim"))]
                say!("error: `sim` needs a simulator build (--features sim); ignoring {kg} kg");
            }
            Command::TestStop => {
                self.test = None;
                self.pending_cal_kg = None;
                self.pending_tare = false;
                say!("stopped");
            }
        }
    }

    async fn start(&mut self, test: Test, prompt: &str) {
        self.test = Some(test);
        say!("{prompt}");
    }

    async fn status(&self) {
        say!("firmware {} on {}", env!("CARGO_PKG_VERSION"), crate::CHIP);
        say!("full scale {} kg; dropped samples {}", self.full_scale_kg, DROPPED.load(Ordering::Relaxed));
        match temperature::latest() {
            Some(t) => say!("temperature {t:.2} C"),
            None => say!("temperature: no probe"),
        }
        for (i, chan) in self.channels.iter().enumerate() {
            let Some(chan) = chan else { continue };
            let mark = if i == self.selected { '*' } else { ' ' };
            let stable = |r: &Reading| if r.stable { ", stable" } else { "" };
            let points = chan.cal.points().len();
            match chan.last {
                Some(r) if chan.calibrated => {
                    say!("{mark}ch{i}: calibrated, {points} points, {:.3} kg (raw {}){}", r.kg, r.raw, stable(&r))
                }
                Some(r) => say!(
                    "{mark}ch{i}: uncalibrated, {points} points, {:.0} counts (raw {}){}",
                    r.filtered,
                    r.raw,
                    stable(&r)
                ),
                None => say!("{mark}ch{i}: {points} points, no readings yet"),
            }
        }
    }

    async fn cal_fit(&mut self) {
        let linearity_kg = Targets::for_full_scale(self.full_scale_kg).linearity_kg;
        let chan = self.selected_mut();
        match chan.cal.fit() {
            Ok(fit) => {
                chan.scale.set_fit(fit);
                chan.calibrated = true;
                say!("cal: {:.6} g/count, intercept {:.3} kg", fit.slope * 1000.0, fit.intercept);
                for (p, res) in chan.cal.residuals(&fit) {
                    say!("  {:>10} counts  {:>9.3} kg  residual {:>+8.1} g", p.counts, p.kg, res * 1000.0);
                }
                say!(
                    "linearity: max residual {:.1} g (target <= {:.1} g): {}. `cal save` to keep it.",
                    fit.max_residual_kg * 1000.0,
                    linearity_kg * 1000.0,
                    verdict(fit.max_residual_kg <= linearity_kg)
                );
            }
            Err(e) => say!("cal: can't fit: {e:?}"),
        }
    }

    async fn cal_show(&mut self) {
        let chan = self.selected_mut();
        if chan.cal.points().is_empty() {
            say!("cal: no points");
        }
        for (i, p) in chan.cal.points().iter().enumerate() {
            say!("  {}: {} counts = {} kg", i + 1, p.counts, p.kg);
        }
        let fit = chan.scale.fit();
        if chan.calibrated {
            say!("active fit: {:.6} g/count, intercept {:.3} kg", fit.slope * 1000.0, fit.intercept);
        } else {
            say!("active fit: none");
        }
    }

    fn selected_mut(&mut self) -> &mut Channel {
        // `selected` only ever points at an active channel.
        self.channels[self.selected].as_mut().expect("selected channel is active")
    }
}

fn verdict(pass: bool) -> &'static str {
    if pass { "PASS" } else { "FAIL" }
}

/// Rounds to the nearest integer (`f64::round` needs std).
fn round(x: f64) -> i32 {
    if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 }
}
