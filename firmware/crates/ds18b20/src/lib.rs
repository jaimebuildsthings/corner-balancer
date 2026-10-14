//! Minimal one-wire driver for one DS18B20 temperature probe per pin.
//!
//! The pin must be open-drain with a pull-up (the probe needs an external
//! 4.7 kΩ to 3.3 V): `set_low` drives the bus low, `set_high` releases it, and
//! `is_high` reads it. With one probe per pin, the driver addresses it with
//! Skip ROM and never searches the bus.
//!
//! A conversion takes up to 750 ms at 12-bit resolution, so it is split in two:
//! [`Ds18b20::start_conversion`], wait (asynchronously, in the firmware), then
//! [`Ds18b20::read_celsius`]. Each bit slot runs in a critical section of at
//! most ~70 µs so interrupts can't stretch its timing.

#![cfg_attr(not(test), no_std)]

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{ErrorType, InputPin, OutputPin};

/// Worst-case conversion time at the power-on default of 12 bits, ms.
pub const CONVERSION_MS: u32 = 750;

const SKIP_ROM: u8 = 0xCC;
const CONVERT_T: u8 = 0x44;
const READ_SCRATCHPAD: u8 = 0xBE;
/// The temperature register's power-on value; seeing it means no conversion has run.
const POWER_ON_RAW: i16 = 0x0550;

/// Driver errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// No device answered the reset pulse: probe missing or pull-up absent.
    NoPresence,
    /// The scratchpad failed its CRC: noise or a loose wire.
    Crc,
    /// The probe reports its power-on value; start a conversion first.
    NotConverted,
    /// The pin failed.
    Pin(E),
}

/// One DS18B20 on an open-drain pin.
pub struct Ds18b20<P, D> {
    pin: P,
    delay: D,
}

type Res<T, P> = Result<T, Error<<P as ErrorType>::Error>>;

impl<P: InputPin + OutputPin, D: DelayNs> Ds18b20<P, D> {
    /// Takes the pin (open-drain, pulled up) and releases the bus.
    pub fn new(mut pin: P, delay: D) -> Res<Self, P> {
        pin.set_high().map_err(Error::Pin)?;
        Ok(Self { pin, delay })
    }

    /// Starts a temperature conversion. Read the result after [`CONVERSION_MS`].
    pub fn start_conversion(&mut self) -> Res<(), P> {
        self.reset()?;
        self.write_byte(SKIP_ROM)?;
        self.write_byte(CONVERT_T)
    }

    /// Reads the last conversion, °C.
    pub fn read_celsius(&mut self) -> Res<f32, P> {
        self.reset()?;
        self.write_byte(SKIP_ROM)?;
        self.write_byte(READ_SCRATCHPAD)?;
        let mut pad = [0u8; 9];
        for b in &mut pad {
            *b = self.read_byte()?;
        }
        parse_scratchpad(&pad)
    }

    /// Returns the pin and delay.
    pub fn release(self) -> (P, D) {
        (self.pin, self.delay)
    }

    /// Reset pulse; returns `NoPresence` if no device pulls the bus low in reply.
    fn reset(&mut self) -> Res<(), P> {
        let Self { pin, delay } = self;
        pin.set_low().map_err(Error::Pin)?;
        delay.delay_us(480);
        let present = critical_section::with(|_| {
            pin.set_high()?;
            delay.delay_us(70);
            pin.is_low()
        })
        .map_err(Error::Pin)?;
        delay.delay_us(410);
        if present { Ok(()) } else { Err(Error::NoPresence) }
    }

    fn write_byte(&mut self, byte: u8) -> Res<(), P> {
        for i in 0..8 {
            let bit = (byte >> i) & 1 == 1;
            let Self { pin, delay } = self;
            critical_section::with(|_| {
                pin.set_low()?;
                delay.delay_us(if bit { 6 } else { 60 });
                pin.set_high()?;
                delay.delay_us(if bit { 64 } else { 10 });
                Ok(())
            })
            .map_err(Error::Pin)?;
        }
        Ok(())
    }

    fn read_byte(&mut self) -> Res<u8, P> {
        let mut byte = 0u8;
        for i in 0..8 {
            let Self { pin, delay } = self;
            let bit = critical_section::with(|_| {
                pin.set_low()?;
                delay.delay_us(6);
                pin.set_high()?;
                delay.delay_us(9);
                let bit = pin.is_high()?;
                delay.delay_us(55);
                Ok(bit)
            })
            .map_err(Error::Pin)?;
            byte |= u8::from(bit) << i;
        }
        Ok(byte)
    }
}

/// Validates a 9-byte scratchpad and decodes its temperature, °C.
pub fn parse_scratchpad<E>(pad: &[u8; 9]) -> Result<f32, Error<E>> {
    if crc8(&pad[..8]) != pad[8] || pad.iter().all(|&b| b == 0xFF) {
        return Err(Error::Crc);
    }
    let raw = i16::from_le_bytes([pad[0], pad[1]]);
    if raw == POWER_ON_RAW {
        return Err(Error::NotConverted);
    }
    Ok(f32::from(raw) / 16.0)
}

/// Dallas/Maxim CRC-8 (polynomial x⁸ + x⁵ + x⁴ + 1, LSB first).
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        let mut b = byte;
        for _ in 0..8 {
            let mix = (crc ^ b) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            b >>= 1;
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    type E = Error<()>;

    fn scratchpad(raw: i16) -> [u8; 9] {
        let [lo, hi] = raw.to_le_bytes();
        let mut pad = [lo, hi, 0x4B, 0x46, 0x7F, 0xFF, 0x0C, 0x10, 0];
        pad[8] = crc8(&pad[..8]);
        pad
    }

    #[test]
    fn crc8_known_vectors() {
        // ROM code from Maxim application note 27.
        assert_eq!(crc8(&[0x02, 0x1C, 0xB8, 0x01, 0x00, 0x00, 0x00]), 0xA2);
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn decodes_datasheet_temperatures() {
        for (raw, want) in
            [(0x07D0, 125.0), (0x0191, 25.0625), (0x0008, 0.5), (0, 0.0), (-0xA2, -10.125), (-0x370, -55.0)]
        {
            assert_eq!(parse_scratchpad::<()>(&scratchpad(raw)), Ok(want), "{raw:#x}");
        }
    }

    #[test]
    fn rejects_bad_scratchpads() {
        let mut pad = scratchpad(0x0191);
        pad[0] ^= 1;
        assert_eq!(parse_scratchpad::<()>(&pad), Err(E::Crc));
        // A floating bus reads all ones.
        assert_eq!(parse_scratchpad::<()>(&[0xFF; 9]), Err(E::Crc));
        assert_eq!(parse_scratchpad::<()>(&scratchpad(POWER_ON_RAW)), Err(E::NotConverted));
    }
}
