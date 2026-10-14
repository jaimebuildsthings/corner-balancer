//! Press-to-assign: find the one pad someone is pressing on.
//!
//! When assignment starts, each pad's current reading becomes its baseline.
//! A pad counts as pressed once it reads `threshold` above its baseline. The
//! press is accepted when exactly one pad is pressed, so leaning on two pads
//! (or the car rocking) doesn't assign the wrong one.

/// Default rise that counts as a press. Uncalibrated pads report counts, which
/// rise far more than this for any real press, so it works for them too.
pub const DEFAULT_THRESHOLD: f32 = 2.0;

/// Outcome of checking the pads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press<K> {
    /// No pad pressed yet.
    None,
    /// Exactly one pad pressed: this one.
    One(K),
    /// More than one pad pressed; wait for the others to be released.
    Many,
}

/// Checks pads given as (key, baseline, current reading).
pub fn detect<K: Copy>(pads: impl IntoIterator<Item = (K, f32, f32)>, threshold: f32) -> Press<K> {
    let mut found = Press::None;
    for (key, baseline, now) in pads {
        if now - baseline >= threshold {
            found = match found {
                Press::None => Press::One(key),
                _ => return Press::Many,
            };
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_single_pressed_pad() {
        let pads = [(0, 10.0, 10.1), (1, 0.0, 15.0), (2, 5.0, 4.0)];
        assert_eq!(detect(pads, DEFAULT_THRESHOLD), Press::One(1));
    }

    #[test]
    fn none_and_many() {
        assert_eq!(detect([(0, 0.0, 1.9)], DEFAULT_THRESHOLD), Press::None);
        assert_eq!(detect([(0, 0.0, 3.0), (1, 0.0, 3.0)], DEFAULT_THRESHOLD), Press::Many);
        assert_eq!(detect::<u8>([], DEFAULT_THRESHOLD), Press::None);
    }

    #[test]
    fn unloading_is_not_a_press() {
        assert_eq!(detect([(0, 20.0, 0.0)], DEFAULT_THRESHOLD), Press::None);
    }
}
