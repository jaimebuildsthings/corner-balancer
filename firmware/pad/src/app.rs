//! The pad's main loop: filters samples, runs console commands and bench tests,
//! streams CSV, and exchanges readings with other pads over ESP-NOW.

use core::sync::atomic::Ordering;

use embassy_time::{Duration, Instant, with_timeout};
use protocol::assign::Press;
use protocol::cmd_msg::{Cmd, Target};
use protocol::command::{HELP, Who};
use protocol::csv;
use protocol::json::{self, PadReading};
use protocol::pad::{Corner, PadName};
use protocol::{Command, ParseError};
use scale::bench::{CreepTest, NoiseTest, PlaceEvent, PlaceRemoveTest, Targets};
use scale::{CalPoint, Calibration, FilterConfig, Fit, Reading, Scale};

use crate::calib_store::{CalibStore, Stored};
use crate::console::LINES;
use crate::link::{INBOX, Incoming, OUTBOX, Outgoing};
use crate::out::say;
use crate::pads::{ASSIGN_TIMEOUT_MS, Pads};
use crate::sampler::{DROPPED, SAMPLES, Sample};
use crate::{MAX_CHANNELS, temperature};

/// Default full-scale rating for pass targets: the phase 1 bench pad.
const DEFAULT_FULL_SCALE_KG: f32 = 200.0;

/// Below this total, percentages are just noise and aren't shown.
const MIN_TOTAL_KG: f32 = 1.0;

/// Interval between scales-protocol frames.
const JSON_INTERVAL_MS: u64 = 200;

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
    json: bool,
    last_json_ms: u64,
    test: Option<Test>,
    pending_cal_kg: Option<f32>,
    pending_tare: bool,
    /// Readings on the selected channel since the last `cal add` or `test zero`.
    fresh: u32,
    store: Option<CalibStore>,
    pads: Pads,
    /// Corner last reported as claimed by another pad too, so the warning prints once.
    warned_conflict: Option<Corner>,
    /// This pad runs the Wi-Fi network and web page.
    host: bool,
}

impl App {
    /// `active` channels get a scale, using the stored calibration where there is one.
    pub fn new(active: usize, store: Option<CalibStore>, stored: Stored, mac: [u8; 6]) -> Self {
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
            json: false,
            last_json_ms: 0,
            test: None,
            pending_cal_kg: None,
            pending_tare: false,
            fresh: 0,
            store,
            pads: Pads::new(mac, stored.corner),
            warned_conflict: None,
            host: stored.host,
        }
    }

    /// Runs forever: handles samples as they arrive and console lines in between.
    pub async fn run(mut self) -> ! {
        say!(
            "corner-balancer {} ({}): pad {}, corner {}. Type `help`.",
            env!("CARGO_PKG_VERSION"),
            crate::CHIP,
            self.pads.name(),
            corner_label(self.pads.corner)
        );
        if self.host {
            say!(
                "web: join Wi-Fi {} and open http://{}/",
                crate::web::ssid(&self.pads.mac).as_str(),
                crate::web::ADDRESS
            );
        }
        if self.store.is_none() {
            say!("warning: no `calib` partition; calibration can't be saved. Flash with partitions.csv.");
        }
        loop {
            if let Ok(sample) = with_timeout(Duration::from_millis(50), SAMPLES.receive()).await {
                self.on_sample(sample).await;
            }
            while let Ok(incoming) = INBOX.try_receive() {
                self.on_incoming(incoming).await;
            }
            while let Ok(line) = LINES.try_receive() {
                self.on_line(line.as_str()).await;
            }
            let now = Instant::now().as_millis();
            self.check_assign(now).await;
            if (self.json || self.host) && now.saturating_sub(self.last_json_ms) >= JSON_INTERVAL_MS {
                self.last_json_ms = now;
                self.send_json(now).await;
            }
            if crate::button::LONG_PRESS.try_take().is_some() {
                self.set_host(!self.host).await;
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
            let msg = self.pads.reading(r.kg, r.raw, r.stable, chan.calibrated, temperature::latest());
            let _ = OUTBOX.try_send(Outgoing::Reading(msg));
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
            Command::Host(on) => self.set_host(on).await,
            Command::Json(on) => {
                self.json = on;
                crate::console::QUIET.store(self.stream || self.json, Ordering::Relaxed);
            }
            Command::Stream(on) => {
                self.stream = on;
                crate::console::QUIET.store(self.stream || self.json, Ordering::Relaxed);
                if on {
                    say!("{}", csv::HEADER);
                }
            }
            Command::Tare => self.request_tare().await,
            Command::TareAll => {
                self.request_tare().await;
                self.send(Target::All, Cmd::Tare);
                say!("tare: sent to every pad");
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
            Command::Pads => self.show_pads().await,
            Command::PadsReset => {
                self.pads.remotes.iter_mut().flatten().for_each(|p| p.stats.reset_counters());
                say!("link counters cleared");
            }
            Command::AssignPress(corner) => {
                let own = self.own_reading().map(|(r, _)| r.kg);
                self.pads.start_assign(corner, Instant::now().as_millis(), own);
                say!(
                    "assign {}: press firmly on the pad under the {} wheel ({} s; `assign stop` cancels)",
                    corner.name(),
                    wheel(corner),
                    ASSIGN_TIMEOUT_MS / 1000
                );
            }
            Command::AssignStop => {
                self.pads.assigning = None;
                say!("assign: cancelled");
            }
            Command::AssignTo(corner, who) => {
                if let Some(mac) = self.resolve(who).await {
                    self.assign(mac, Some(corner)).await;
                }
            }
            Command::Unassign(who) => {
                if let Some(mac) = self.resolve(who).await {
                    self.assign(mac, None).await;
                }
            }
            Command::Blink(who) => match self.resolve(who).await {
                Some(mac) if mac == self.pads.mac => {
                    crate::led::LED.signal(crate::led::Pattern::Find);
                    say!("blinking this pad");
                }
                Some(mac) => {
                    self.send(Target::Pad(mac), Cmd::Blink);
                    say!("blinking pad {}", PadName::from_mac(&mac));
                }
                None => {}
            },
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
        say!(
            "firmware {} on {}; pad {}, corner {}",
            env!("CARGO_PKG_VERSION"),
            crate::CHIP,
            self.pads.name(),
            corner_label(self.pads.corner)
        );
        say!("full scale {} kg; dropped samples {}", self.full_scale_kg, DROPPED.load(Ordering::Relaxed));
        if self.host {
            say!(
                "host: Wi-Fi {} (open), page at http://{}/",
                crate::web::ssid(&self.pads.mac).as_str(),
                crate::web::ADDRESS
            );
        } else {
            say!("host: off (`host on`, or hold BOOT 5 s)");
        }
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

    /// Saves host mode, confirms on the LED and restarts, since Wi-Fi is set up at boot.
    async fn set_host(&mut self, on: bool) {
        if let Some(Err(e)) = self.store.as_mut().map(|s| s.save_host(on)) {
            say!("error: host setting not saved: {e:?}");
            return;
        }
        crate::led::LED.signal(if on { crate::led::Pattern::HostOn } else { crate::led::Pattern::HostOff });
        if on {
            say!("host on: restarting as Wi-Fi network {}", crate::web::ssid(&self.pads.mac).as_str());
        } else {
            say!("host off: restarting");
        }
        embassy_time::Timer::after_millis(3_000).await;
        esp_hal::system::software_reset()
    }

    /// This pad's channel-0 reading and whether it is calibrated.
    fn own_reading(&self) -> Option<(Reading, bool)> {
        self.channels[0].as_ref().and_then(|c| c.last.map(|r| (r, c.calibrated)))
    }

    async fn request_tare(&mut self) {
        self.pending_tare = true;
        self.fresh = 0;
        say!("tare: waiting for a stable reading...");
    }

    /// Queues a command for other pads.
    fn send(&mut self, target: Target, cmd: Cmd) {
        let msg = self.pads.command(target, cmd);
        let _ = OUTBOX.try_send(Outgoing::Command(msg));
    }

    async fn save_corner(&mut self) {
        let corner = self.pads.corner;
        match self.store.as_mut().map(|s| s.save_corner(corner)) {
            Some(Ok(())) => {}
            Some(Err(e)) => say!("warning: corner not saved: {e:?}"),
            None => say!("warning: corner not saved: no calib partition"),
        }
    }

    /// Finds a pad named in a command, explaining if it can't.
    async fn resolve(&self, who: Who) -> Option<[u8; 6]> {
        let found = self.pads.resolve(who, Instant::now().as_millis());
        if found.is_none() {
            match who {
                Who::Name(name) => say!("error: pad {name} not heard in the last 2 s; see `pads`"),
                Who::Corner(c) => say!("error: no pad is {} right now; see `pads`", c.name()),
                Who::This => {}
            }
        }
        found
    }

    /// Gives `corner` (or none) to the pad `target`, here and on every other pad.
    async fn assign(&mut self, target: [u8; 6], corner: Option<Corner>) {
        if self.pads.apply_assign(target, corner) {
            self.save_corner().await;
        }
        // Always broadcast: the target takes the corner and any other holder gives it up.
        self.send(Target::Pad(target), Cmd::Assign(corner));
        say!("pad {} is now {}", PadName::from_mac(&target), corner_label(corner));
    }

    /// Advances a press-to-assign.
    async fn check_assign(&mut self, now_ms: u64) {
        let Some(a) = &self.pads.assigning else { return };
        let corner = a.corner;
        if now_ms.saturating_sub(a.started_ms) > ASSIGN_TIMEOUT_MS {
            self.pads.assigning = None;
            say!("assign {}: no press detected; try again", corner.name());
            return;
        }
        let own = self.own_reading().map(|(r, _)| r.kg);
        match self.pads.check_assign(now_ms, own) {
            Press::One(mac) => {
                self.pads.assigning = None;
                self.assign(mac, Some(corner)).await;
                if mac == self.pads.mac {
                    crate::led::LED.signal(crate::led::Pattern::Find);
                } else {
                    self.send(Target::Pad(mac), Cmd::Blink);
                }
            }
            Press::Many => {
                if let Some(a) = self.pads.assigning.as_mut()
                    && !a.warned_many
                {
                    a.warned_many = true;
                    say!("assign {}: more than one pad pressed; press only one", corner.name());
                }
            }
            Press::None => {}
        }
    }

    async fn on_incoming(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Reading { src, msg, t_ms } => {
                self.pads.heard(src, msg, t_ms);
                let clash = msg.corner.is_some() && msg.corner == self.pads.corner;
                if clash && self.warned_conflict != msg.corner {
                    self.warned_conflict = msg.corner;
                    say!(
                        "warning: pad {} also claims {}; run `assign {}` to settle it",
                        PadName::from_mac(&src),
                        corner_label(msg.corner),
                        corner_label(msg.corner)
                    );
                }
            }
            Incoming::Command { src, msg } => {
                if !self.pads.dedup.first_time(&src, msg.id) {
                    return;
                }
                let from = PadName::from_mac(&src);
                match msg.cmd {
                    Cmd::Assign(corner) => {
                        let Target::Pad(target) = msg.target else { return };
                        if self.pads.apply_assign(target, corner) {
                            self.save_corner().await;
                            say!("this pad is now {} (set from pad {from})", corner_label(self.pads.corner));
                        }
                    }
                    Cmd::Tare if msg.is_for(&self.pads.mac) => {
                        say!("tare requested by pad {from}");
                        self.request_tare().await;
                    }
                    Cmd::Blink if msg.is_for(&self.pads.mac) => crate::led::LED.signal(crate::led::Pattern::Find),
                    Cmd::Tare | Cmd::Blink => {}
                }
            }
        }
    }

    /// How many fresh pads (this one included) claim each corner.
    fn corner_claims(&self, now_ms: u64) -> [u8; 4] {
        let mut claims = [0u8; 4];
        let corners = core::iter::once(self.pads.corner).chain(self.pads.fresh_remotes(now_ms).map(|p| p.msg.corner));
        for c in corners.flatten() {
            claims[usize::from(c.index())] += 1;
        }
        claims
    }

    /// Latest weight per corner from fresh, calibrated pads (this one included),
    /// and whether all of those are stable. A corner claimed by two pads is
    /// left out: there is no way to know which reading is right.
    fn corner_weights(&self, now_ms: u64) -> ([Option<f32>; 4], bool) {
        let claims = self.corner_claims(now_ms);
        let mut w = [None; 4];
        let mut all_stable = true;
        let mut put = |corner: Option<Corner>, kg: f32, stable: bool| {
            if let Some(c) = corner
                && claims[usize::from(c.index())] == 1
            {
                w[usize::from(c.index())] = Some(kg);
                all_stable &= stable;
            }
        };
        if let Some((r, true)) = self.own_reading() {
            put(self.pads.corner, r.kg, r.stable);
        }
        for p in self.pads.fresh_remotes(now_ms).filter(|p| p.msg.calibrated) {
            put(p.msg.corner, p.msg.kg, p.msg.stable);
        }
        (w, all_stable)
    }

    /// One scales-protocol frame: this pad plus every other pad heard recently.
    async fn send_json(&self, now_ms: u64) {
        let mut pads =
            [PadReading { name: self.pads.name(), corner: None, kg: 0.0, stable: false, calibrated: false, age_ms: 0 };
                8];
        let mut n = 0;
        if let Some((r, calibrated)) = self.own_reading() {
            pads[0] = PadReading {
                name: self.pads.name(),
                corner: self.pads.corner,
                kg: r.kg,
                stable: r.stable,
                calibrated,
                age_ms: 0,
            };
            n = 1;
        }
        for p in self.pads.fresh_remotes(now_ms).take(pads.len() - n) {
            pads[n] = PadReading {
                name: p.name(),
                corner: p.msg.corner,
                kg: p.msg.kg,
                stable: p.msg.stable,
                calibrated: p.msg.calibrated,
                age_ms: p.stats.age_ms(now_ms).unwrap_or(0),
            };
            n += 1;
        }
        let mut buf = crate::out::Buf::new();
        let _ = json::write_weights(&mut buf, now_ms, &pads[..n]);
        if self.host {
            crate::web::publish(buf.as_bytes());
        }
        if self.json {
            crate::out::line(buf.as_bytes()).await;
        }
    }

    async fn show_pads(&self) {
        let now = Instant::now().as_millis();
        say!("pad      corner        kg  stable    age  received  lost %  max gap");
        let uncal = |calibrated: bool| if calibrated { "" } else { "  UNCALIBRATED: counts" };
        match self.own_reading() {
            Some((r, calibrated)) => say!(
                "{}* {:<6} {:>10.3}  {:<6}  (this pad){}",
                self.pads.name(),
                corner_label(self.pads.corner),
                r.kg,
                yes_no(r.stable),
                uncal(calibrated)
            ),
            None => say!("{}* {:<6}  (this pad, no readings yet)", self.pads.name(), corner_label(self.pads.corner)),
        }
        for p in self.pads.remotes.iter().flatten() {
            let age = p.stats.age_ms(now).unwrap_or(0);
            say!(
                "{}  {:<6} {:>10.3}  {:<6} {:>5.1}s  {:>8}  {:>6.2}  {:>5.2}s{}{}",
                p.name(),
                corner_label(p.msg.corner),
                p.msg.kg,
                yes_no(p.msg.stable),
                age as f32 / 1000.0,
                p.stats.received,
                p.stats.loss_pct().unwrap_or(0.0),
                p.stats.max_gap_ms as f32 / 1000.0,
                if p.is_fresh(now) { "" } else { "  STALE" },
                uncal(p.msg.calibrated)
            );
        }
        for (c, n) in Corner::ALL.into_iter().zip(self.corner_claims(now)) {
            if n > 1 {
                say!("CONFLICT: {n} pads claim {}; left out of totals. Fix with `assign {}`.", c.name(), c.name());
            }
        }
        let (w, all_stable) = self.corner_weights(now);
        let settling = if all_stable { "" } else { "  (settling)" };
        let (present, count) = present_corners(&w);
        match (w, count) {
            ([Some(lf), Some(rf), Some(lr), Some(rr)], _) if lf + rf + lr + rr >= MIN_TOTAL_KG => {
                if let Some(c) = scale::corners(lf, rf, lr, rr) {
                    say!(
                        "total {:.3} kg  front {:.2} %  left {:.2} %  cross (LF+RR) {:.2} %{settling}",
                        c.total,
                        c.front_pct,
                        c.left_pct,
                        c.cross_pct
                    );
                }
            }
            (_, 2 | 4) if w.iter().flatten().sum::<f32>() < MIN_TOTAL_KG => {
                let total: f32 = w.iter().flatten().sum();
                say!("total {total:.3} kg (percentages from {MIN_TOTAL_KG} kg){settling}");
            }
            (_, 2) => {
                let (a, b) = (present[0], present[1]);
                let (ka, kb) = (w[usize::from(a.index())].unwrap_or(0.0), w[usize::from(b.index())].unwrap_or(0.0));
                match scale::split(ka, kb) {
                    Some(pct) => say!("total {:.3} kg  {} carries {:.2} %{settling}", ka + kb, b.name(), pct),
                    None => say!("total {:.3} kg{settling}", ka + kb),
                }
            }
            _ => say!("(totals need two or four pads with corners, heard recently and calibrated)"),
        }
    }

    fn selected_mut(&mut self) -> &mut Channel {
        // `selected` only ever points at an active channel.
        self.channels[self.selected].as_mut().expect("selected channel is active")
    }
}

/// The corners that have a weight, in LF, RF, LR, RR order, and how many.
fn present_corners(w: &[Option<f32>; 4]) -> ([Corner; 4], usize) {
    let mut out = [Corner::LF; 4];
    let mut n = 0;
    for c in Corner::ALL.into_iter().filter(|c| w[usize::from(c.index())].is_some()) {
        out[n] = c;
        n += 1;
    }
    (out, n)
}

fn corner_label(c: Option<Corner>) -> &'static str {
    c.map_or("-", Corner::name)
}

fn wheel(c: Corner) -> &'static str {
    match c {
        Corner::LF => "left-front",
        Corner::RF => "right-front",
        Corner::LR => "left-rear",
        Corner::RR => "right-rear",
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
