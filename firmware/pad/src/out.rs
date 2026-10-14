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

/// Writes one complete line of output. If the user is part-way through typing
/// a command, the half-typed line is cleared first and redrawn after, so the
/// message never lands in the middle of it.
pub async fn line(text: &[u8]) {
    const CLEAR_LINE: &[u8] = b"\r\x1b[K";
    let mut out = [0u8; 256 + 2 + 3 + crate::console::LINE_MAX];
    let mut n = 0;
    let mut push = |bytes: &[u8]| {
        let k = bytes.len().min(out.len() - n);
        out[n..n + k].copy_from_slice(&bytes[..k]);
        n += k;
    };
    let partial = crate::console::visible_partial();
    if partial.is_some() {
        push(CLEAR_LINE);
    }
    push(text);
    push(b"\r\n");
    if let Some(p) = &partial {
        push(p.as_bytes());
    }
    write(&out[..n]).await;
}

/// Formats one line and writes it with [`line`].
macro_rules! say {
    ($($arg:tt)*) => {{
        let mut buf = $crate::out::Buf::new();
        let _ = core::fmt::Write::write_fmt(&mut buf, format_args!($($arg)*));
        $crate::out::line(buf.as_bytes()).await;
    }};
}
pub(crate) use say;
