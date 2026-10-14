//! Commands from one pad to others over ESP-NOW: assign a corner, tare, blink.
//!
//! Broadcasts aren't acknowledged, so the sender repeats each command a few
//! times with the same `id`, and receivers drop repeats with [`Dedup`].

use crate::pad::{Corner, corner_from_byte, corner_to_byte};

/// Who a command is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Every pad that hears it.
    All,
    /// The pad with this MAC address.
    Pad([u8; 6]),
}

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// The target takes this corner (or none). Any *other* pad holding that
    /// corner gives it up, so two pads never share one.
    Assign(Option<Corner>),
    /// Zero at the next settled reading.
    Tare,
    /// Flash the pad's LED so someone can find it.
    Blink,
}

/// One command frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CmdMsg {
    /// Repeats of one command share an ID.
    pub id: u16,
    /// Recipient.
    pub target: Target,
    /// Action.
    pub cmd: Cmd,
}

/// Why a command frame was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdDecodeError {
    /// Not a command frame: wrong length or magic.
    NotOurs,
    /// Unknown version or command kind.
    Unsupported,
}

const MAGIC: [u8; 2] = *b"CC";
const VERSION: u8 = 1;
const ALL: [u8; 6] = [0xFF; 6];

impl CmdMsg {
    /// Encoded size in bytes.
    pub const LEN: usize = 13;

    /// Frame: magic "CC", version, kind, target MAC (all FF = everyone), id, argument.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let (kind, arg) = match self.cmd {
            Cmd::Assign(c) => (1, corner_to_byte(c)),
            Cmd::Tare => (2, 0),
            Cmd::Blink => (3, 0),
        };
        let target = match self.target {
            Target::All => ALL,
            Target::Pad(mac) => mac,
        };
        let mut b = [0u8; Self::LEN];
        b[0..2].copy_from_slice(&MAGIC);
        b[2] = VERSION;
        b[3] = kind;
        b[4..10].copy_from_slice(&target);
        b[10..12].copy_from_slice(&self.id.to_le_bytes());
        b[12] = arg;
        b
    }

    /// Parses a frame produced by [`CmdMsg::encode`].
    pub fn decode(b: &[u8]) -> Result<Self, CmdDecodeError> {
        let b: &[u8; Self::LEN] = b.try_into().map_err(|_| CmdDecodeError::NotOurs)?;
        if b[0..2] != MAGIC {
            return Err(CmdDecodeError::NotOurs);
        }
        if b[2] != VERSION {
            return Err(CmdDecodeError::Unsupported);
        }
        let cmd = match b[3] {
            1 => Cmd::Assign(corner_from_byte(b[12])),
            2 => Cmd::Tare,
            3 => Cmd::Blink,
            _ => return Err(CmdDecodeError::Unsupported),
        };
        let mac: [u8; 6] = b[4..10].try_into().unwrap();
        let target = if mac == ALL { Target::All } else { Target::Pad(mac) };
        Ok(Self { id: u16::from_le_bytes([b[10], b[11]]), target, cmd })
    }

    /// True if this command is addressed to the pad with MAC `me`.
    pub fn is_for(&self, me: &[u8; 6]) -> bool {
        match self.target {
            Target::All => true,
            Target::Pad(mac) => mac == *me,
        }
    }
}

/// Remembers recent (sender, id) pairs so repeated commands run once.
#[derive(Clone, Debug, Default)]
pub struct Dedup {
    seen: [Option<([u8; 6], u16)>; 8],
    next: usize,
}

impl Dedup {
    /// Returns true the first time a (sender, id) pair is seen.
    pub fn first_time(&mut self, src: &[u8; 6], id: u16) -> bool {
        if self.seen.iter().flatten().any(|(s, i)| s == src && *i == id) {
            return false;
        }
        self.seen[self.next] = Some((*src, id));
        self.next = (self.next + 1) % self.seen.len();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [1, 2, 3, 4, 5, 6];

    #[test]
    fn round_trips() {
        let cases = [
            CmdMsg { id: 7, target: Target::Pad(MAC), cmd: Cmd::Assign(Some(Corner::RR)) },
            CmdMsg { id: 8, target: Target::Pad(MAC), cmd: Cmd::Assign(None) },
            CmdMsg { id: 9, target: Target::All, cmd: Cmd::Tare },
            CmdMsg { id: 0xFFFF, target: Target::Pad(MAC), cmd: Cmd::Blink },
        ];
        for m in cases {
            assert_eq!(CmdMsg::decode(&m.encode()), Ok(m));
        }
    }

    #[test]
    fn rejects_other_frames() {
        let mut b = CmdMsg { id: 1, target: Target::All, cmd: Cmd::Tare }.encode();
        assert_eq!(CmdMsg::decode(&b[..12]), Err(CmdDecodeError::NotOurs));
        b[3] = 99;
        assert_eq!(CmdMsg::decode(&b), Err(CmdDecodeError::Unsupported));
        b[0] = b'C';
        b[1] = b'B'; // a PadMsg's magic
        assert_eq!(CmdMsg::decode(&b), Err(CmdDecodeError::NotOurs));
    }

    #[test]
    fn addressing() {
        let to_me = CmdMsg { id: 1, target: Target::Pad(MAC), cmd: Cmd::Blink };
        assert!(to_me.is_for(&MAC));
        assert!(!to_me.is_for(&[9; 6]));
        assert!(CmdMsg { id: 1, target: Target::All, cmd: Cmd::Tare }.is_for(&[9; 6]));
    }

    #[test]
    fn dedup_runs_repeats_once() {
        let mut d = Dedup::default();
        assert!(d.first_time(&MAC, 1));
        assert!(!d.first_time(&MAC, 1));
        assert!(d.first_time(&MAC, 2));
        assert!(d.first_time(&[9; 6], 1), "same id from another pad is new");
        // Old entries age out after eight newer ones.
        for i in 10..18 {
            d.first_time(&MAC, i);
        }
        assert!(d.first_time(&MAC, 1));
    }
}
