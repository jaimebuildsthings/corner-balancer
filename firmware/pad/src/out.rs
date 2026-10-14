//! Serial output shared by the console echo and the app.

use core::fmt;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pipe::Pipe;
use esp_hal::Async;
use esp_hal::uart::UartTx;

/// Bytes waiting for the UART. Writers wait when it is full, so output is never
/// reordered within one write, but a slow host throttles them.
static PIPE: Pipe<CriticalSectionRawMutex, 1024> = Pipe::new();

/// Moves queued output to the UART. The transmitter stays with this task
/// because async drivers can't be shared between cores.
#[embassy_executor::task]
pub async fn task(mut tx: UartTx<'static, Async>) {
    let mut buf = [0u8; 128];
    loop {
        let n = PIPE.read(&mut buf).await;
        let mut rest = &buf[..n];
        while !rest.is_empty() {
            match tx.write_async(rest).await {
                Ok(0) | Err(_) => break,
                Ok(written) => rest = &rest[written..],
            }
        }
    }
}

/// Queues bytes for the serial port.
pub async fn write(bytes: &[u8]) {
    PIPE.write_all(bytes).await;
}

/// A fixed-size line buffer; text past its capacity is dropped.
pub struct Buf {
    data: [u8; 256],
    len: usize,
}

impl Buf {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self { data: [0; 256], len: 0 }
    }

    /// The buffered bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len]
    }
}

impl fmt::Write for Buf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.data.len() - self.len);
        self.data[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Formats one line (adding `\r\n`) and writes it.
macro_rules! say {
    ($($arg:tt)*) => {{
        let mut buf = $crate::out::Buf::new();
        let _ = core::fmt::Write::write_fmt(&mut buf, format_args!($($arg)*));
        let _ = core::fmt::Write::write_str(&mut buf, "\r\n");
        $crate::out::write(buf.as_bytes()).await;
    }};
}
pub(crate) use say;
