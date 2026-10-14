//! Line input from the serial console, with echo and backspace.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use esp_hal::Async;
use esp_hal::uart::UartRx;

use crate::out;

/// Longest accepted line.
pub const LINE_MAX: usize = 64;

/// One line typed at the console.
#[derive(Clone, Copy)]
pub struct Line {
    data: [u8; LINE_MAX],
    len: usize,
}

impl Line {
    /// The line as text; invalid UTF-8 reads as empty.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..self.len]).unwrap_or("")
    }
}

/// Completed lines for the app.
pub static LINES: Channel<CriticalSectionRawMutex, Line, 4> = Channel::new();

/// Reads bytes, echoes them, and queues each line on Enter.
#[embassy_executor::task]
pub async fn task(mut rx: UartRx<'static, Async>) {
    let mut line = Line { data: [0; LINE_MAX], len: 0 };
    let mut buf = [0u8; 32];
    loop {
        let Ok(n) = rx.read_async(&mut buf).await else { continue };
        for &b in &buf[..n] {
            match b {
                b'\r' | b'\n' => {
                    out::write(b"\r\n").await;
                    if line.len > 0 {
                        // Drop the line rather than block if the app is busy.
                        let _ = LINES.try_send(line);
                        line.len = 0;
                    }
                }
                0x08 | 0x7F if line.len > 0 => {
                    line.len -= 1;
                    out::write(b"\x08 \x08").await;
                }
                0x20..=0x7E if line.len < LINE_MAX => {
                    line.data[line.len] = b;
                    line.len += 1;
                    out::write(&[b]).await;
                }
                _ => {}
            }
        }
    }
}
