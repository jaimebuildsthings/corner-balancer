//! Who a pad is (its name, from its MAC address) and where it sits (its corner).

use core::fmt;

/// The wheel a pad sits under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// Left front.
    LF = 0,
    /// Right front.
    RF = 1,
    /// Left rear.
    LR = 2,
    /// Right rear.
    RR = 3,
}

impl Corner {
    /// All corners, in index order.
    pub const ALL: [Corner; 4] = [Corner::LF, Corner::RF, Corner::LR, Corner::RR];

    /// Index 0–3, as used on the wire.
    pub const fn index(self) -> u8 {
        self as u8
    }

    /// The corner for an index, if valid.
    pub fn from_index(i: u8) -> Option<Self> {
        Self::ALL.get(usize::from(i)).copied()
    }

    /// Short name: "LF", "RF", "LR" or "RR".
    pub const fn name(self) -> &'static str {
        match self {
            Corner::LF => "LF",
            Corner::RF => "RF",
            Corner::LR => "LR",
            Corner::RR => "RR",
        }
    }

    /// Parses a short name, ignoring case.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name().eq_ignore_ascii_case(s))
    }
}

/// Wire encoding of an optional corner: its index, or this for none.
pub const NO_CORNER: u8 = 0xFF;

/// Encodes an optional corner as one byte.
pub fn corner_to_byte(c: Option<Corner>) -> u8 {
    c.map_or(NO_CORNER, Corner::index)
}

/// Decodes one byte; anything but 0–3 means no corner.
pub fn corner_from_byte(b: u8) -> Option<Corner> {
    Corner::from_index(b)
}

/// A pad's short name: the last three bytes of its MAC address, as six hex
/// digits (e.g. `9a91a8`). Printed on a sticker on the pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadName(pub [u8; 3]);

impl PadName {
    /// The name of the pad with this MAC address.
    pub fn from_mac(mac: &[u8; 6]) -> Self {
        Self([mac[3], mac[4], mac[5]])
    }

    /// True if `mac` belongs to the pad with this name.
    pub fn matches(&self, mac: &[u8; 6]) -> bool {
        mac[3..] == self.0
    }

    /// Parses six hex digits, ignoring case.
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 6 {
            return None;
        }
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        let mut out = [0u8; 3];
        for (i, pair) in b.chunks_exact(2).enumerate() {
            out[i] = hex(pair[0])? << 4 | hex(pair[1])?;
        }
        Some(Self(out))
    }
}

impl fmt::Display for PadName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners() {
        assert_eq!(Corner::parse("lf"), Some(Corner::LF));
        assert_eq!(Corner::parse("RR"), Some(Corner::RR));
        assert_eq!(Corner::parse("front"), None);
        for c in Corner::ALL {
            assert_eq!(corner_from_byte(corner_to_byte(Some(c))), Some(c));
        }
        assert_eq!(corner_to_byte(None), NO_CORNER);
        assert_eq!(corner_from_byte(4), None);
    }

    #[test]
    fn names() {
        let mac = [0x00, 0x4b, 0x12, 0x9a, 0x91, 0xa8];
        let name = PadName::from_mac(&mac);
        assert_eq!(name.to_string(), "9a91a8");
        assert_eq!(PadName::parse("9A91a8"), Some(name));
        assert!(name.matches(&mac));
        assert!(!name.matches(&[0; 6]));
        assert_eq!(PadName::parse("9a91a"), None);
        assert_eq!(PadName::parse("9a91ag"), None);
    }
}
