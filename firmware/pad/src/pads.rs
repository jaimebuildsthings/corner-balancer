//! What this pad knows about the set: its own name and corner, the other pads
//! it hears (keyed by MAC address), and a press-to-assign in progress.

use protocol::PadMsg;
use protocol::assign::{self, Press};
use protocol::cmd_msg::{Cmd, CmdMsg, Dedup, Target};
use protocol::command::Who;
use protocol::link::LinkStats;
use protocol::pad::{Corner, PadName};

/// A pad not heard from for this long is stale: shown as such, left out of totals.
pub const STALE_MS: u64 = 2_000;
/// How long a press-to-assign waits.
pub const ASSIGN_TIMEOUT_MS: u64 = 30_000;
/// Other pads tracked at once (a set is four; spares may be on the bench).
const MAX_REMOTES: usize = 7;

/// The latest reading from another pad and how well its link is doing.
pub struct Remote {
    /// Sender's MAC address.
    pub mac: [u8; 6],
    /// Its latest reading.
    pub msg: PadMsg,
    /// Link quality.
    pub stats: LinkStats,
}

impl Remote {
    /// The pad's name.
    pub fn name(&self) -> PadName {
        PadName::from_mac(&self.mac)
    }

    /// Heard within [`STALE_MS`].
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        self.stats.age_ms(now_ms).is_some_and(|age| age <= STALE_MS)
    }
}

/// A press-to-assign in progress.
pub struct Assigning {
    /// The corner being given out.
    pub corner: Corner,
    /// When it started, ms since boot.
    pub started_ms: u64,
    /// This pad's weight when it started.
    own_baseline: Option<f32>,
    /// Each remote's weight when it started.
    baselines: [Option<([u8; 6], f32)>; MAX_REMOTES],
    /// "Release the other pads" has been said.
    pub warned_many: bool,
}

/// The set as this pad sees it.
pub struct Pads {
    /// This pad's MAC address.
    pub mac: [u8; 6],
    /// This pad's corner.
    pub corner: Option<Corner>,
    seq: u32,
    next_cmd_id: u16,
    /// Other pads heard.
    pub remotes: [Option<Remote>; MAX_REMOTES],
    /// Repeats of commands already handled.
    pub dedup: Dedup,
    /// Press-to-assign in progress.
    pub assigning: Option<Assigning>,
}

impl Pads {
    /// This pad, with its saved corner.
    pub fn new(mac: [u8; 6], corner: Option<Corner>) -> Self {
        Self {
            mac,
            corner,
            seq: 0,
            next_cmd_id: 0,
            remotes: Default::default(),
            dedup: Dedup::default(),
            assigning: None,
        }
    }

    /// This pad's name.
    pub fn name(&self) -> PadName {
        PadName::from_mac(&self.mac)
    }

    /// The next reading to broadcast.
    pub fn reading(&mut self, kg: f32, raw: i32, stable: bool, calibrated: bool, temp_c: Option<f32>) -> PadMsg {
        let msg = PadMsg { corner: self.corner, seq: self.seq, kg, raw, stable, calibrated, temp_c };
        self.seq = self.seq.wrapping_add(1);
        msg
    }

    /// A new command with a fresh ID.
    pub fn command(&mut self, target: Target, cmd: Cmd) -> CmdMsg {
        self.next_cmd_id = self.next_cmd_id.wrapping_add(1);
        CmdMsg { id: self.next_cmd_id, target, cmd }
    }

    /// Records a reading from another pad. Returns false if there was no room for a new pad.
    pub fn heard(&mut self, mac: [u8; 6], msg: PadMsg, t_ms: u64) -> bool {
        let slot = match self.remotes.iter().position(|r| r.as_ref().is_some_and(|r| r.mac == mac)) {
            Some(i) => i,
            None => match self.remotes.iter().position(Option::is_none) {
                Some(i) => {
                    self.remotes[i] = Some(Remote { mac, msg, stats: LinkStats::default() });
                    i
                }
                None => return false,
            },
        };
        let remote = self.remotes[slot].as_mut().expect("slot filled above");
        remote.msg = msg;
        remote.stats.record(msg.seq, t_ms);
        true
    }

    /// Applies an assignment locally: if it is for this pad, take the corner;
    /// otherwise give up that corner if this pad holds it. Also updates the
    /// cached corner of the remote, so displays change before its next reading.
    /// Returns true if this pad's own corner changed.
    pub fn apply_assign(&mut self, target: [u8; 6], corner: Option<Corner>) -> bool {
        let before = self.corner;
        if target == self.mac {
            self.corner = corner;
        } else if corner.is_some() && self.corner == corner {
            self.corner = None;
        }
        for r in self.remotes.iter_mut().flatten() {
            if r.mac == target {
                r.msg.corner = corner;
            } else if corner.is_some() && r.msg.corner == corner {
                r.msg.corner = None;
            }
        }
        self.corner != before
    }

    /// The MAC address of a pad named in a command, if it is known and fresh.
    pub fn resolve(&self, who: Who, now_ms: u64) -> Option<[u8; 6]> {
        match who {
            Who::This => Some(self.mac),
            Who::Name(name) if name.matches(&self.mac) => Some(self.mac),
            Who::Corner(c) if self.corner == Some(c) => Some(self.mac),
            Who::Name(name) => self.fresh_remotes(now_ms).find(|r| name.matches(&r.mac)).map(|r| r.mac),
            Who::Corner(c) => self.fresh_remotes(now_ms).find(|r| r.msg.corner == Some(c)).map(|r| r.mac),
        }
    }

    /// Remotes heard within [`STALE_MS`].
    pub fn fresh_remotes(&self, now_ms: u64) -> impl Iterator<Item = &Remote> {
        self.remotes.iter().flatten().filter(move |r| r.is_fresh(now_ms))
    }

    /// Starts a press-to-assign with every pad's current weight as its baseline.
    pub fn start_assign(&mut self, corner: Corner, now_ms: u64, own_kg: Option<f32>) {
        let mut baselines = [None; MAX_REMOTES];
        for (slot, r) in baselines.iter_mut().zip(self.fresh_remotes(now_ms)) {
            *slot = Some((r.mac, r.msg.kg));
        }
        self.assigning =
            Some(Assigning { corner, started_ms: now_ms, own_baseline: own_kg, baselines, warned_many: false });
    }

    /// Checks a press-to-assign against current weights.
    pub fn check_assign(&self, now_ms: u64, own_kg: Option<f32>) -> Press<[u8; 6]> {
        let Some(a) = &self.assigning else { return Press::None };
        let own = a.own_baseline.zip(own_kg).map(|(b, now)| (self.mac, b, now));
        let others = a.baselines.iter().flatten().filter_map(|&(mac, base)| {
            self.fresh_remotes(now_ms).find(|r| r.mac == mac).map(|r| (mac, base, r.msg.kg))
        });
        assign::detect(own.into_iter().chain(others), assign::DEFAULT_THRESHOLD)
    }
}
