//! ESP-NOW between pads. Every pad broadcasts its readings and listens for
//! the others'; there is no pairing. All pads use one fixed channel, which the
//! UI pad's Wi-Fi access point will share (phase 1b).

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Instant;
use esp_radio::esp_now::{BROADCAST_ADDRESS, EspNowReceiver, EspNowSender};
use protocol::PadMsg;

/// The Wi-Fi channel all pads use.
pub const CHANNEL: u8 = 1;

/// Readings from this pad, waiting to be broadcast.
pub static OUTBOX: Channel<CriticalSectionRawMutex, PadMsg, 8> = Channel::new();

/// A reading from another pad and when it arrived.
#[derive(Clone, Copy)]
pub struct Heard {
    /// The decoded message.
    pub msg: PadMsg,
    /// Arrival, ms since boot.
    pub t_ms: u64,
}

/// Readings heard from other pads, for the app.
pub static INBOX: Channel<CriticalSectionRawMutex, Heard, 16> = Channel::new();

/// Broadcasts queued readings. Broadcasts aren't acknowledged, so a lost frame
/// just shows up as a sequence gap on the receiving side.
#[embassy_executor::task]
pub async fn sender(mut tx: EspNowSender) {
    loop {
        let msg = OUTBOX.receive().await;
        let _ = tx.send_async(&BROADCAST_ADDRESS, &msg.encode()).await;
    }
}

/// Decodes frames from other pads; anything that isn't a `PadMsg` is dropped.
#[embassy_executor::task]
pub async fn receiver(mut rx: EspNowReceiver) {
    loop {
        let frame = rx.receive_async().await;
        if let Ok(msg) = PadMsg::decode(frame.data()) {
            let _ = INBOX.try_send(Heard { msg, t_ms: Instant::now().as_millis() });
        }
    }
}
