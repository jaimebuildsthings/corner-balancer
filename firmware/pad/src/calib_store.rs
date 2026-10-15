//! Calibration points and pad settings in the `calib` flash partition.
//!
//! The partition holds two 4 KB slots in the crash-safe record format from
//! `protocol::record`; each save goes to the slot not holding the newest record.
//! The payload is, per channel, a length byte followed by that channel's
//! `Calibration::encode` bytes, then an optional settings block
//! `b'S', corner, flags` (corner 0–3 = LF, RF, LR, RR, anything else =
//! unassigned; flags bit 0 = host). Older records without flags still load.
//! Records written before the settings block existed still load.

use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;
use protocol::pad::{Corner, corner_from_byte, corner_to_byte};
use protocol::record;
use scale::Calibration;
use scale::calibration::ENCODED_MAX_LEN;

use crate::MAX_CHANNELS;

const SLOT_LEN: u32 = FlashStorage::SECTOR_SIZE;
const SETTINGS_LEN: usize = 3;
const FLAG_HOST: u8 = 1 << 0;
const PAYLOAD_MAX: usize = MAX_CHANNELS * (1 + ENCODED_MAX_LEN) + SETTINGS_LEN;
const RECORD_MAX: usize = record::OVERHEAD + PAYLOAD_MAX;

/// Why the store couldn't be used.
#[derive(Debug)]
pub enum StoreError {
    /// The partition table couldn't be read.
    PartitionTable,
    /// No partition labelled `calib` of at least two sectors.
    NoPartition,
    /// A flash read or write failed.
    Flash,
}

/// Everything kept in the partition.
#[derive(Clone, Default)]
pub struct Stored {
    /// Calibration points per channel.
    pub cals: [Calibration; MAX_CHANNELS],
    /// This pad's corner, if assigned.
    pub corner: Option<Corner>,
    /// This pad runs the Wi-Fi access point and web page.
    pub host: bool,
}

/// Calibration and settings storage.
pub struct CalibStore {
    flash: FlashStorage<'static>,
    offset: u32,
    /// Slot and sequence number of the newest record, if any.
    newest: Option<(usize, u32)>,
    /// What is in flash now, so saving one part keeps the other.
    saved: Stored,
}

impl CalibStore {
    /// Finds the `calib` partition.
    pub fn open(mut flash: FlashStorage<'static>) -> Result<Self, StoreError> {
        let mut table_buf = [0u8; partitions::PARTITION_TABLE_MAX_LEN];
        let table =
            partitions::read_partition_table(&mut flash, &mut table_buf).map_err(|_| StoreError::PartitionTable)?;
        let part = table
            .iter()
            .find(|p| p.label_as_str() == "calib" && p.len() >= 2 * SLOT_LEN)
            .ok_or(StoreError::NoPartition)?;
        Ok(Self { offset: part.offset(), flash, newest: None, saved: Stored::default() })
    }

    /// Loads everything. Missing parts come back as defaults.
    pub fn load(&mut self) -> Result<Stored, StoreError> {
        let mut slots = [[0u8; RECORD_MAX]; 2];
        for (i, slot) in slots.iter_mut().enumerate() {
            self.flash.read(self.slot_addr(i), slot).map_err(|_| StoreError::Flash)?;
        }
        let mut stored = Stored::default();
        self.newest = None;
        if let Some((slot, seq, payload)) = record::newest(&[&slots[0], &slots[1]]) {
            self.newest = Some((slot, seq));
            let mut rest = payload;
            for cal in &mut stored.cals {
                let Some((&len, tail)) = rest.split_first() else { break };
                let Some(bytes) = tail.get(..usize::from(len)) else { break };
                *cal = Calibration::decode(bytes).unwrap_or_default();
                rest = &tail[usize::from(len)..];
            }
            if let [b'S', corner, rest @ ..] = rest {
                stored.corner = corner_from_byte(*corner);
                stored.host = rest.first().is_some_and(|f| f & FLAG_HOST != 0);
            }
        }
        self.saved = stored.clone();
        Ok(stored)
    }

    /// Saves calibration points, keeping the stored settings.
    pub fn save_cals(&mut self, cals: &[Calibration; MAX_CHANNELS]) -> Result<(), StoreError> {
        let next = Stored { cals: cals.clone(), ..self.saved.clone() };
        self.save(next)
    }

    /// Saves the corner, keeping the stored calibration.
    pub fn save_corner(&mut self, corner: Option<Corner>) -> Result<(), StoreError> {
        let next = Stored { corner, ..self.saved.clone() };
        self.save(next)
    }

    /// Saves the host setting, keeping everything else.
    pub fn save_host(&mut self, host: bool) -> Result<(), StoreError> {
        let next = Stored { host, ..self.saved.clone() };
        self.save(next)
    }

    fn save(&mut self, stored: Stored) -> Result<(), StoreError> {
        let mut payload = [0u8; PAYLOAD_MAX];
        let mut len = 0;
        for cal in &stored.cals {
            let mut enc = [0u8; ENCODED_MAX_LEN];
            let n = cal.encode(&mut enc);
            payload[len] = n as u8;
            payload[len + 1..len + 1 + n].copy_from_slice(&enc[..n]);
            len += 1 + n;
        }
        let flags = if stored.host { FLAG_HOST } else { 0 };
        payload[len..len + SETTINGS_LEN].copy_from_slice(&[b'S', corner_to_byte(stored.corner), flags]);
        len += SETTINGS_LEN;

        let (slot, seq) = match self.newest {
            Some((slot, seq)) => (1 - slot, seq.wrapping_add(1)),
            None => (0, 1),
        };
        let mut rec = [0u8; RECORD_MAX];
        let n = record::encode(seq, &payload[..len], &mut rec).ok_or(StoreError::Flash)?;
        self.flash.write(self.slot_addr(slot), &rec[..n]).map_err(|_| StoreError::Flash)?;
        self.newest = Some((slot, seq));
        self.saved = stored;
        Ok(())
    }

    fn slot_addr(&self, slot: usize) -> u32 {
        self.offset + slot as u32 * SLOT_LEN
    }
}
