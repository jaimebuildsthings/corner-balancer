//! ESP-NOW between pads. Every pad broadcasts its readings and listens for
//! the others'; there is no pairing. Commands (assign, tare, blink) travel the
//! same way, addressed by MAC, and are repeated because broadcasts aren't
//! acknowledged. All pads use one fixed channel.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Instant, Timer};
use esp_radio::esp_now::{BROADCAST_ADDRESS, EspNowReceiver, EspNowSender};
use protocol::PadMsg;
use protocol::cmd_msg::CmdMsg;

/// The Wi-Fi channel all pads use.
pub const CHANNEL: u8 = 1;

/// Times each command is sent; receivers run it once.
const COMMAND_REPEATS: usize = 3;

/// A frame to broadcast.
pub enum Outgoing {
    /// This pad's latest reading.
    Reading(PadMsg),
    /// A command for one or all pads.
    Command(CmdMsg),
}

/// Frames waiting to be broadcast.
pub static OUTBOX: Channel<CriticalSectionRawMutex, Outgoing, 8> = Channel::new();

/// A frame from another pad.
pub enum Incoming {
    /// A reading, with its sender and arrival time (ms since boot).
    Reading { src: [u8; 6], msg: PadMsg, t_ms: u64 },
    /// A command, with its sender.
    Command { src: [u8; 6], msg: CmdMsg },
}

/// Frames heard from other pads, for the app.
pub static INBOX: Channel<CriticalSectionRawMutex, Incoming, 16> = Channel::new();

/// Broadcasts queued frames.
#[embassy_executor::task]
pub async fn sender(mut tx: EspNowSender) {
    loop {
        match OUTBOX.receive().await {
            Outgoing::Reading(msg) => {
                let _ = tx.send_async(&BROADCAST_ADDRESS, &msg.encode()).await;
            }
            Outgoing::Command(msg) => {
                let frame = msg.encode();
                for _ in 0..COMMAND_REPEATS {
                    let _ = tx.send_async(&BROADCAST_ADDRESS, &frame).await;
                    Timer::after_millis(15).await;
                }
            }
        }
    }
}

/// Decodes frames from other pads; anything else is dropped.
#[embassy_executor::task]
pub async fn receiver(mut rx: EspNowReceiver) {
    loop {
        let frame = rx.receive_async().await;
        let src = frame.info.src_address;
        let incoming = if let Ok(msg) = PadMsg::decode(frame.data()) {
            Incoming::Reading { src, msg, t_ms: Instant::now().as_millis() }
        } else if let Ok(msg) = CmdMsg::decode(frame.data()) {
            Incoming::Command { src, msg }
        } else {
            continue;
        };
        let _ = INBOX.try_send(incoming);
    }
}
