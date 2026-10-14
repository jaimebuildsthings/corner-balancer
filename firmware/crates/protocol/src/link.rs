//! Link quality for one remote pad, from the sequence numbers and arrival times
//! of its [`PadMsg`](crate::PadMsg)s.

/// Running link statistics for one sender.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    last: Option<(u32, u64)>,
    /// Messages received.
    pub received: u32,
    /// Messages missed, from gaps in the sequence numbers.
    pub lost: u32,
    /// Longest time between two received messages, ms.
    pub max_gap_ms: u64,
}

impl LinkStats {
    /// Records a message with sequence number `seq` arriving at `t_ms`.
    ///
    /// A sequence number at or below the last one means the sender restarted;
    /// counting starts afresh from it without recording a loss.
    pub fn record(&mut self, seq: u32, t_ms: u64) {
        if let Some((last_seq, last_t)) = self.last {
            if seq > last_seq {
                self.lost += seq - last_seq - 1;
            }
            self.max_gap_ms = self.max_gap_ms.max(t_ms.saturating_sub(last_t));
        }
        self.received += 1;
        self.last = Some((seq, t_ms));
    }

    /// Lost messages as a percentage of those sent, or `None` before any arrive.
    pub fn loss_pct(&self) -> Option<f32> {
        let sent = self.received + self.lost;
        (sent > 0).then(|| self.lost as f32 / sent as f32 * 100.0)
    }

    /// Milliseconds since the last message, or `None` before any arrive.
    pub fn age_ms(&self, now_ms: u64) -> Option<u64> {
        self.last.map(|(_, t)| now_ms.saturating_sub(t))
    }

    /// Clears the counters but keeps the last message, so the next gap is measured correctly.
    pub fn reset_counters(&mut self) {
        *self = Self { last: self.last, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_losses_and_gaps() {
        let mut s = LinkStats::default();
        assert_eq!(s.loss_pct(), None);
        assert_eq!(s.age_ms(5), None);
        s.record(10, 1_000);
        s.record(11, 1_100);
        s.record(14, 1_400); // 12 and 13 lost
        assert_eq!((s.received, s.lost, s.max_gap_ms), (3, 2, 300));
        assert!((s.loss_pct().unwrap() - 40.0).abs() < 1e-4);
        assert_eq!(s.age_ms(1_450), Some(50));
    }

    #[test]
    fn sender_restart_is_not_a_loss() {
        let mut s = LinkStats::default();
        s.record(500, 0);
        s.record(0, 2_000);
        s.record(1, 2_100);
        assert_eq!((s.received, s.lost, s.max_gap_ms), (3, 0, 2_000));
    }

    #[test]
    fn reset_keeps_timing_reference() {
        let mut s = LinkStats::default();
        s.record(1, 0);
        s.record(5, 3_000);
        s.reset_counters();
        assert_eq!((s.received, s.lost, s.max_gap_ms), (0, 0, 0));
        s.record(6, 3_100);
        assert_eq!((s.received, s.lost, s.max_gap_ms), (1, 0, 100));
    }
}
