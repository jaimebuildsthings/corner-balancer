//! Bit-banged driver for the HX711 24-bit load cell amplifier.
//!
//! The HX711 signals a finished conversion by pulling DOUT low. The host then
//! clocks 24 data bits out on SCK, plus 1–3 extra pulses that pick the input
//! and gain for the *next* conversion. Holding SCK high for more than 60 µs
//! powers the chip down, so each read runs inside a critical section: an
//! interrupt landing mid-pulse would otherwise stretch it and corrupt the read.
//! With the 1 µs half-period used here, the critical section lasts ~55 µs.

#![cfg_attr(not(test), no_std)]

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};

/// Input channel and gain, applied from the conversion after the next read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gain {
    /// Channel A, gain 128 (±20 mV at 5 V excitation). The power-on default.
    #[default]
    A128,
    /// Channel B, gain 32.
    B32,
    /// Channel A, gain 64.
    A64,
}

impl Gain {
    /// Total SCK pulses per read that select this gain.
    const fn pulses(self) -> u8 {
        match self {
            Gain::A128 => 25,
            Gain::B32 => 26,
            Gain::A64 => 27,
        }
    }
}

/// Driver errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<InE, OutE> {
    /// No conversion is ready (DOUT is high).
    NotReady,
    /// Reading DOUT failed.
    Input(InE),
    /// Driving SCK failed.
    Output(OutE),
}

/// Largest value the HX711 reports; it saturates here on overload.
pub const MAX: i32 = 0x7F_FFFF;
/// Smallest value the HX711 reports; it saturates here on overload.
pub const MIN: i32 = -0x80_0000;

/// True when `raw` is pinned at either end of the range, which means the
/// input is overloaded or a wire is disconnected.
pub const fn is_saturated(raw: i32) -> bool {
    raw == MAX || raw == MIN
}

/// Half-period of SCK. The datasheet allows 0.2–50 µs high; 1 µs leaves margin
/// for slow GPIO without approaching the 60 µs power-down threshold.
const HALF_PERIOD_NS: u32 = 1_000;
/// Time SCK is held high to power down (datasheet: > 60 µs).
const POWER_DOWN_NS: u32 = 100_000;

/// One HX711 on two GPIOs.
pub struct Hx711<Dout, Sck, D> {
    dout: Dout,
    sck: Sck,
    delay: D,
    gain: Gain,
}

type Res<T, Dout, Sck> = Result<
    T,
    Error<<Dout as embedded_hal::digital::ErrorType>::Error, <Sck as embedded_hal::digital::ErrorType>::Error>,
>;

impl<Dout: InputPin, Sck: OutputPin, D: DelayNs> Hx711<Dout, Sck, D> {
    /// Takes the pins and drives SCK low, which also wakes the chip if it was powered down.
    ///
    /// `gain` takes effect from the second conversion; the chip starts on [`Gain::A128`].
    pub fn new(dout: Dout, mut sck: Sck, delay: D, gain: Gain) -> Res<Self, Dout, Sck> {
        sck.set_low().map_err(Error::Output)?;
        Ok(Self { dout, sck, delay, gain })
    }

    /// True when a conversion is waiting to be read.
    pub fn is_ready(&mut self) -> Res<bool, Dout, Sck> {
        self.dout.is_low().map_err(Error::Input)
    }

    /// Reads the waiting conversion as a signed 24-bit value.
    ///
    /// Returns [`Error::NotReady`] if DOUT is high; poll [`Hx711::is_ready`]
    /// (or wait for DOUT's falling edge) first.
    pub fn read(&mut self) -> Res<i32, Dout, Sck> {
        if !self.is_ready()? {
            return Err(Error::NotReady);
        }
        let pulses = self.gain.pulses();
        let Self { dout, sck, delay, .. } = self;
        let raw = critical_section::with(|_| {
            let mut value: u32 = 0;
            for i in 0..pulses {
                sck.set_high().map_err(Error::Output)?;
                delay.delay_ns(HALF_PERIOD_NS);
                // DOUT updates on SCK's rising edge, so it is valid now.
                if i < 24 {
                    value = (value << 1) | u32::from(dout.is_high().map_err(Error::Input)?);
                }
                sck.set_low().map_err(Error::Output)?;
                delay.delay_ns(HALF_PERIOD_NS);
            }
            Ok(value)
        })?;
        // Sign-extend the 24-bit two's-complement value.
        Ok(((raw << 8) as i32) >> 8)
    }

    /// Selects the input and gain for the conversion after the next read.
    pub fn set_gain(&mut self, gain: Gain) {
        self.gain = gain;
    }

    /// The gain requested by the most recent [`Hx711::set_gain`] or [`Hx711::new`].
    pub fn gain(&self) -> Gain {
        self.gain
    }

    /// Powers the chip down (about 1 µA).
    pub fn power_down(&mut self) -> Res<(), Dout, Sck> {
        self.sck.set_low().map_err(Error::Output)?;
        self.sck.set_high().map_err(Error::Output)?;
        self.delay.delay_ns(POWER_DOWN_NS);
        Ok(())
    }

    /// Wakes the chip. It resets to [`Gain::A128`] for its first conversion,
    /// and the first reading after wake-up should be discarded while it settles.
    pub fn power_up(&mut self) -> Res<(), Dout, Sck> {
        self.sck.set_low().map_err(Error::Output)
    }

    /// Returns the pins and delay.
    pub fn release(self) -> (Dout, Sck, D) {
        (self.dout, self.sck, self.delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A simulated HX711 that shifts out `value` and checks SCK timing.
    #[derive(Default)]
    struct Chip {
        ready: bool,
        value: u32,
        edges: u32,
        sck_high: bool,
        high_ns: u32,
        max_high_ns: u32,
        powered_down: bool,
    }

    impl Chip {
        fn dout(&self) -> bool {
            if self.edges == 0 {
                !self.ready
            } else if self.edges <= 24 {
                (self.value >> (24 - self.edges)) & 1 == 1
            } else {
                true
            }
        }
    }

    type Shared = Rc<RefCell<Chip>>;
    struct Dout(Shared);
    struct Sck(Shared);
    struct Delay(Shared);

    impl embedded_hal::digital::ErrorType for Dout {
        type Error = Infallible;
    }
    impl InputPin for Dout {
        fn is_high(&mut self) -> Result<bool, Infallible> {
            Ok(self.0.borrow().dout())
        }
        fn is_low(&mut self) -> Result<bool, Infallible> {
            Ok(!self.0.borrow().dout())
        }
    }
    impl embedded_hal::digital::ErrorType for Sck {
        type Error = Infallible;
    }
    impl OutputPin for Sck {
        fn set_high(&mut self) -> Result<(), Infallible> {
            let mut c = self.0.borrow_mut();
            if !c.sck_high {
                c.edges += 1;
                c.high_ns = 0;
            }
            c.sck_high = true;
            Ok(())
        }
        fn set_low(&mut self) -> Result<(), Infallible> {
            self.0.borrow_mut().sck_high = false;
            Ok(())
        }
    }
    impl DelayNs for Delay {
        fn delay_ns(&mut self, ns: u32) {
            let mut c = self.0.borrow_mut();
            if c.sck_high {
                c.high_ns += ns;
                c.max_high_ns = c.max_high_ns.max(c.high_ns);
                if c.high_ns > 60_000 {
                    c.powered_down = true;
                }
            }
        }
    }

    fn setup(value: u32, ready: bool, gain: Gain) -> (Hx711<Dout, Sck, Delay>, Shared) {
        let chip = Rc::new(RefCell::new(Chip { ready, value, ..Chip::default() }));
        let hx = Hx711::new(Dout(chip.clone()), Sck(chip.clone()), Delay(chip.clone()), gain).unwrap();
        (hx, chip)
    }

    #[test]
    fn reads_and_sign_extends() {
        for (bits, want) in [(0x00_0000, 0), (0x00_0001, 1), (0x7F_FFFF, MAX), (0x80_0000, MIN), (0xFF_FFFF, -1)] {
            let (mut hx, _) = setup(bits, true, Gain::A128);
            assert_eq!(hx.read(), Ok(want), "{bits:#x}");
        }
        let (mut hx, _) = setup(0xFE_1DC0, true, Gain::A128);
        assert_eq!(hx.read(), Ok(-123_456));
    }

    #[test]
    fn not_ready_does_not_clock() {
        let (mut hx, chip) = setup(5, false, Gain::A128);
        assert_eq!(hx.is_ready(), Ok(false));
        assert_eq!(hx.read(), Err(Error::NotReady));
        assert_eq!(chip.borrow().edges, 0);
    }

    #[test]
    fn gain_selects_pulse_count() {
        for (gain, pulses) in [(Gain::A128, 25), (Gain::B32, 26), (Gain::A64, 27)] {
            let (mut hx, chip) = setup(0x12_3456, true, gain);
            assert_eq!(hx.read(), Ok(0x12_3456));
            assert_eq!(chip.borrow().edges, pulses);
            assert!(!chip.borrow().sck_high, "SCK must end low");
        }
    }

    #[test]
    fn sck_pulses_stay_short() {
        let (mut hx, chip) = setup(0x55_5555, true, Gain::A64);
        hx.read().unwrap();
        let c = chip.borrow();
        assert!(c.max_high_ns <= 2_000, "{} ns", c.max_high_ns);
        assert!(!c.powered_down);
    }

    #[test]
    fn power_down_holds_sck_high() {
        let (mut hx, chip) = setup(0, true, Gain::A128);
        hx.power_down().unwrap();
        assert!(chip.borrow().powered_down);
        hx.power_up().unwrap();
        assert!(!chip.borrow().sck_high);
    }

    #[test]
    fn saturation() {
        assert!(is_saturated(MAX));
        assert!(is_saturated(MIN));
        assert!(!is_saturated(0));
        assert!(!is_saturated(MAX - 1));
    }

    #[test]
    fn set_gain_applies_to_next_read() {
        let (mut hx, chip) = setup(1, true, Gain::A128);
        hx.set_gain(Gain::B32);
        assert_eq!(hx.gain(), Gain::B32);
        hx.read().unwrap();
        assert_eq!(chip.borrow().edges, 26);
    }
}
