//! Line input from the serial console, with echo and backspace.

use core::sync::atomic::{AtomicBool, Ordering};

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

/// Set while CSV is streaming: keystrokes aren't echoed (they would land inside
/// CSV rows); the whole line is echoed on Enter instead.
pub static QUIET: AtomicBool = AtomicBool::new(false);

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
                    if QUIET.load(Ordering::Relaxed) {
                        if line.len > 0 {
                            // One write, so it can't be split by a CSV row.
                            let mut echo = [0u8; LINE_MAX + 4];
                            echo[..2].copy_from_slice(b"> ");
                            echo[2..2 + line.len].copy_from_slice(&line.data[..line.len]);
                            echo[2 + line.len..4 + line.len].copy_from_slice(b"\r\n");
                            out::write(&echo[..4 + line.len]).await;
                        }
                    } else {
                        out::write(b"\r\n").await;
                    }
                    if line.len > 0 {
                        // Drop the line rather than block if the app is busy.
                        let _ = LINES.try_send(line);
                        line.len = 0;
                    }
                }
                0x08 | 0x7F if line.len > 0 => {
                    line.len -= 1;
                    if !QUIET.load(Ordering::Relaxed) {
                        out::write(b"\x08 \x08").await;
                    }
                }
                0x20..=0x7E if line.len < LINE_MAX => {
                    line.data[line.len] = b;
                    line.len += 1;
                    if !QUIET.load(Ordering::Relaxed) {
                        out::write(&[b]).await;
                    }
                }
                _ => {}
            }
        }
    }
}
