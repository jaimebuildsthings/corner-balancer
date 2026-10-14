//! The pad's main loop: filters samples, runs console commands and bench tests,
//! streams CSV, and exchanges readings with other pads over ESP-NOW.

use core::sync::atomic::Ordering;

use embassy_time::{Duration, Instant, with_timeout};
use protocol::command::{HELP, MAX_PAD_ID};
use protocol::csv;
use protocol::link::LinkStats;
use protocol::{Command, PadMsg, ParseError};
use scale::bench::{CreepTest, NoiseTest, PlaceEvent, PlaceRemoveTest, Targets};
use scale::{CalPoint, Calibration, FilterConfig, Fit, Reading, Scale};

use crate::calib_store::CalibStore;
use crate::console::LINES;
use crate::link::{Heard, INBOX, OUTBOX};
use crate::out::say;
use crate::sampler::{DROPPED, SAMPLES, Sample};
use crate::{MAX_CHANNELS, temperature};

/// Default full-scale rating for pass targets: the phase 1 bench pad.
const DEFAULT_FULL_SCALE_KG: f32 = 200.0;

/// A pad not heard from for this long is shown as stale and left out of totals.
const STALE_MS: u64 = 2_000;

const PADS: usize = MAX_PAD_ID as usize + 1;

/// The latest reading from another pad and how well its link is doing.
struct RemotePad {
    msg: PadMsg,
    stats: LinkStats,
}

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
    pad_id: u8,
    seq: u32,
    remotes: [Option<RemotePad>; PADS],
    warned_duplicate_id: bool,
}

impl App {
    /// `active` channels get a scale, using the stored calibration where there is one.
    pub fn new(active: usize, mut store: Option<CalibStore>) -> Self {
        let stored = store.as_mut().and_then(|s| s.load().ok()).unwrap_or_default();
        let channels = core::array::from_fn(|i| {
            (i < active).then(|| {
                let cal = stored.cals[i].clone();
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
            pad_id: stored.pad_id.min(MAX_PAD_ID),
            seq: 0,
            remotes: Default::default(),
            warned_duplicate_id: false,
        }
    }

    /// Runs forever: handles samples as they arrive and console lines in between.
    pub async fn run(mut self) -> ! {
        say!(
            "corner-balancer pad {} ({}), pad id {}. Type `help`.",
            env!("CARGO_PKG_VERSION"),
            crate::CHIP,
            self.pad_id
        );
        if self.store.is_none() {
            say!("warning: no `calib` partition; calibration can't be saved. Flash with partitions.csv.");
        }
        loop {
            if let Ok(sample) = with_timeout(Duration::from_millis(50), SAMPLES.receive()).await {
                self.on_sample(sample).await;
            }
            while let Ok(heard) = INBOX.try_receive() {
                self.on_heard(heard).await;
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
        if ch == 0 {
            // Channel 0 is this pad's reading for the other pads.
            let msg = PadMsg {
                pad_id: self.pad_id,
                seq: self.seq,
                kg: r.kg,
                raw: r.raw,
                stable: r.stable,
                calibrated: chan.calibrated,
                temp_c: temperature::latest(),
            };
            self.seq = self.seq.wrapping_add(1);
            let _ = OUTBOX.try_send(msg);
        }
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
                match self.store.as_mut().map(|s| s.save_cals(&cals)) {
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
            Command::Id(id) => {
                self.pad_id = id;
                self.seq = 0;
                self.warned_duplicate_id = false;
                match self.store.as_mut().map(|s| s.save_pad_id(id)) {
                    Some(Ok(())) => say!("pad id {id} (saved)"),
                    Some(Err(e)) => say!("pad id {id}, but saving failed: {e:?}"),
                    None => say!("pad id {id} (not saved: no calib partition)"),
                }
            }
            Command::Pads => self.show_pads().await,
            Command::PadsReset => {
                self.remotes.iter_mut().flatten().for_each(|p| p.stats.reset_counters());
                say!("link counters cleared");
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
        say!("firmware {} on {}; pad id {}", env!("CARGO_PKG_VERSION"), crate::CHIP, self.pad_id);
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

    async fn on_heard(&mut self, heard: Heard) {
        let id = heard.msg.pad_id;
        if id == self.pad_id {
            if !self.warned_duplicate_id {
                self.warned_duplicate_id = true;
                say!("warning: another pad also uses id {id}; give each pad its own with `id N`");
            }
            return;
        }
        let Some(slot) = self.remotes.get_mut(usize::from(id)) else { return };
        let remote = slot.get_or_insert(RemotePad { msg: heard.msg, stats: LinkStats::default() });
        remote.msg = heard.msg;
        remote.stats.record(heard.msg.seq, heard.t_ms);
    }

    /// Every pad's latest weight, this one included; `None` if unheard, stale
    /// or uncalibrated (an uncalibrated pad reports counts, not kg).
    fn fresh_weights(&self, now_ms: u64) -> [Option<f32>; PADS] {
        core::array::from_fn(|id| {
            if id == usize::from(self.pad_id) {
                let chan = self.channels[0].as_ref().filter(|c| c.calibrated)?;
                chan.last.map(|r| r.kg)
            } else {
                self.remotes[id]
                    .as_ref()
                    .filter(|p| p.msg.calibrated && p.stats.age_ms(now_ms).is_some_and(|age| age <= STALE_MS))
                    .map(|p| p.msg.kg)
            }
        })
    }

    async fn show_pads(&self) {
        let now = Instant::now().as_millis();
        say!("pad        kg  stable    age  received  lost %  max gap");
        for id in 0..PADS {
            if id == usize::from(self.pad_id) {
                match self.channels[0].as_ref().and_then(|c| c.last.map(|r| (r, c.calibrated))) {
                    Some((r, calibrated)) => say!(
                        "{id}* {:>10.3}  {:<6}  (this pad){}",
                        r.kg,
                        yes_no(r.stable),
                        if calibrated { "" } else { "  UNCALIBRATED: counts" }
                    ),
                    None => say!("{id}*  (this pad, no readings yet)"),
                }
                continue;
            }
            let Some(p) = &self.remotes[id] else { continue };
            let age = p.stats.age_ms(now).unwrap_or(0);
            say!(
                "{id}  {:>10.3}  {:<6} {:>5.1}s  {:>8}  {:>6.2}  {:>5.2}s{}{}",
                p.msg.kg,
                yes_no(p.msg.stable),
                age as f32 / 1000.0,
                p.stats.received,
                p.stats.loss_pct().unwrap_or(0.0),
                p.stats.max_gap_ms as f32 / 1000.0,
                if age > STALE_MS { "  STALE" } else { "" },
                if p.msg.calibrated { "" } else { "  UNCALIBRATED: counts" }
            );
        }
        let w = self.fresh_weights(now);
        match w {
            [Some(lf), Some(rf), Some(lr), Some(rr)] => {
                if let Some(c) = scale::corners(lf, rf, lr, rr) {
                    say!(
                        "total {:.3} kg  front {:.2} %  left {:.2} %  cross (LF+RR) {:.2} %",
                        c.total,
                        c.front_pct,
                        c.left_pct,
                        c.cross_pct
                    );
                }
            }
            [Some(a), Some(b), None, None] => match scale::split(a, b) {
                Some(pct) => say!("total {:.3} kg  pad 1 carries {:.2} %", a + b, pct),
                None => say!("total {:.3} kg", a + b),
            },
            _ => say!("(totals need pads 0+1 or all four: heard, fresh and calibrated)"),
        }
    }

    fn selected_mut(&mut self) -> &mut Channel {
        // `selected` only ever points at an active channel.
        self.channels[self.selected].as_mut().expect("selected channel is active")
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn verdict(pass: bool) -> &'static str {
    if pass { "PASS" } else { "FAIL" }
}

/// Rounds to the nearest integer (`f64::round` needs std).
fn round(x: f64) -> i32 {
    if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 }
}
