//! Calibration points in the `calib` flash partition.
//!
//! The partition holds two 4 KB slots in the crash-safe record format from
//! `protocol::record`; each save goes to the slot not holding the newest record.
//! The payload is, per channel, a length byte followed by that channel's
//! `Calibration::encode` bytes.

use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;
use protocol::record;
use scale::Calibration;
use scale::calibration::ENCODED_MAX_LEN;

use crate::MAX_CHANNELS;

const SLOT_LEN: u32 = FlashStorage::SECTOR_SIZE;
const PAYLOAD_MAX: usize = MAX_CHANNELS * (1 + ENCODED_MAX_LEN);
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

/// Calibration storage for all channels.
pub struct CalibStore {
    flash: FlashStorage<'static>,
    offset: u32,
    /// Slot and sequence number of the newest record, if any.
    newest: Option<(usize, u32)>,
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
        Ok(Self { offset: part.offset(), flash, newest: None })
    }

    /// Loads every channel's points. Channels with nothing stored come back empty.
    pub fn load(&mut self) -> Result<[Calibration; MAX_CHANNELS], StoreError> {
        let mut slots = [[0u8; RECORD_MAX]; 2];
        for (i, slot) in slots.iter_mut().enumerate() {
            self.flash.read(self.slot_addr(i), slot).map_err(|_| StoreError::Flash)?;
        }
        let mut cals: [Calibration; MAX_CHANNELS] = Default::default();
        self.newest = None;
        if let Some((slot, seq, payload)) = record::newest(&[&slots[0], &slots[1]]) {
            self.newest = Some((slot, seq));
            let mut rest = payload;
            for cal in &mut cals {
                let Some((&len, tail)) = rest.split_first() else { break };
                let Some(bytes) = tail.get(..usize::from(len)) else { break };
                *cal = Calibration::decode(bytes).unwrap_or_default();
                rest = &tail[usize::from(len)..];
            }
        }
        Ok(cals)
    }

    /// Saves every channel's points to the older slot.
    pub fn save(&mut self, cals: &[Calibration; MAX_CHANNELS]) -> Result<(), StoreError> {
        let mut payload = [0u8; PAYLOAD_MAX];
        let mut len = 0;
        for cal in cals {
            let mut enc = [0u8; ENCODED_MAX_LEN];
            let n = cal.encode(&mut enc);
            payload[len] = n as u8;
            payload[len + 1..len + 1 + n].copy_from_slice(&enc[..n]);
            len += 1 + n;
        }
        let (slot, seq) = match self.newest {
            Some((slot, seq)) => (1 - slot, seq.wrapping_add(1)),
            None => (0, 1),
        };
        let mut rec = [0u8; RECORD_MAX];
        let n = record::encode(seq, &payload[..len], &mut rec).ok_or(StoreError::Flash)?;
        self.flash.write(self.slot_addr(slot), &rec[..n]).map_err(|_| StoreError::Flash)?;
        self.newest = Some((slot, seq));
        Ok(())
    }

    fn slot_addr(&self, slot: usize) -> u32 {
        self.offset + slot as u32 * SLOT_LEN
    }
}
