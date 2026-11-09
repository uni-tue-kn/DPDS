const L1_OVERHEAD: u32 = 7 + 1 + 12; // preamble + SFD + IPG

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LimiterMode {
    Spacer,
    TokenBucketFilter,
}

#[derive(Debug, Copy, Clone)]
pub struct Limiter {
    pub mode: LimiterMode,
    pub activate: bool,
    pub rate_bpns: f64,      // bits per nanosecond
    pub last_update_ns: f64, // ns
    pub limit_bit: f64,      // bits
    pub fill_state_bit: f64, // bits (fill state for Spacer, tokens for TBF)
}

impl Limiter {
    pub fn new(mode: LimiterMode, activate: bool, rate_bps: f64, limit_bit: f64) -> Limiter {
        let rate_bpns = rate_bps / 1e9;

        let fill_state_bit = match mode {
            LimiterMode::Spacer => 0.,                   // Spacer starts empty
            LimiterMode::TokenBucketFilter => limit_bit, // TBF starts full
        };

        Limiter {
            mode,
            activate,
            rate_bpns,
            last_update_ns: 0.,
            limit_bit,
            fill_state_bit,
        }
    }

    /// Returns the additional send offset (in ns) for a packet of the given
    /// size, or `None` if the packet exceeds the configured rate limit and
    /// should be dropped.
    #[inline]
    pub fn limiter_calculate_offset_ns(
        &mut self,
        time_to_sent_ns: f64,
        packet_length_byte: u32,
    ) -> Option<f64> {
        if !self.activate {
            return Some(0.);
        }

        let duration_since_last_update_ns = (time_to_sent_ns - self.last_update_ns).max(0.);

        self.last_update_ns = f64::max(self.last_update_ns, time_to_sent_ns);

        // The packet length excludes the L1 overhead.
        let packet_size_in_bit = (f64::from(packet_length_byte) + f64::from(L1_OVERHEAD)) * 8.;

        match self.mode {
            LimiterMode::Spacer => self.spacer_calculate_offset_ns(
                time_to_sent_ns,
                duration_since_last_update_ns,
                packet_size_in_bit,
            ),
            LimiterMode::TokenBucketFilter => {
                self.tbf_packet_in_profile(duration_since_last_update_ns, packet_size_in_bit)
            }
        }
    }

    /// What [`Limiter::limiter_calculate_offset_ns`] would return, leaving the
    /// limiter unchanged (it runs the same code on a copy).
    pub fn limiter_peek_offset_ns(
        &self,
        time_to_sent_ns: f64,
        packet_length_byte: u32,
    ) -> Option<f64> {
        let mut probe = *self;
        probe.limiter_calculate_offset_ns(time_to_sent_ns, packet_length_byte)
    }

    /// Spacer: schedules packets by calculating a send offset based on the fill state.
    fn spacer_calculate_offset_ns(
        &mut self,
        time_to_sent_ns: f64,
        duration_since_last_update_ns: f64,
        packet_size_in_bit: f64,
    ) -> Option<f64> {
        // Drain the bits that could be sent since the last update.
        self.fill_state_bit = f64::max(
            0.,
            self.fill_state_bit - duration_since_last_update_ns * self.rate_bpns,
        );

        if self.fill_state_bit + packet_size_in_bit > self.limit_bit {
            None
        } else {
            let offset_between_time_to_sent_and_last_update =
                (self.last_update_ns - time_to_sent_ns).max(0.);

            // Time until the queued bits (without this packet) are sent, plus
            // the gap from time_to_sent to last_update so the offset is
            // relative to time_to_sent.
            let time_offset_ns = (self.fill_state_bit / self.rate_bpns)
                + offset_between_time_to_sent_and_last_update;

            self.fill_state_bit += packet_size_in_bit;

            Some(time_offset_ns)
        }
    }

    /// TBF: checks if a packet is within the token budget and consumes tokens.
    fn tbf_packet_in_profile(
        &mut self,
        duration_since_last_update_ns: f64,
        packet_size_in_bit: f64,
    ) -> Option<f64> {
        // Refill proportionally to the elapsed time, capped at the limit.
        self.fill_state_bit = f64::min(
            self.limit_bit,
            self.fill_state_bit + (duration_since_last_update_ns * self.rate_bpns),
        );

        if self.fill_state_bit >= packet_size_in_bit {
            self.fill_state_bit -= packet_size_in_bit;
            Some(0.)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deactivated_limiter_never_delays_or_drops() {
        let mut l = Limiter::new(LimiterMode::Spacer, false, 1e9, 1000.);
        assert_eq!(l.limiter_calculate_offset_ns(0., 100), Some(0.));
    }

    #[test]
    fn tbf_admits_within_budget_and_drops_oversized_burst() {
        // rate = 1 bit/ns, bucket = 100_000 bit (starts full)
        let mut l = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 100_000.);
        // A 100-byte packet (plus overhead) fits and is admitted with no offset.
        assert_eq!(l.limiter_calculate_offset_ns(0., 100), Some(0.));

        // A tiny bucket cannot hold a single packet -> dropped.
        let mut small = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 100.);
        assert_eq!(small.limiter_calculate_offset_ns(0., 100), None);
    }

    #[test]
    fn peek_matches_calculate_and_leaves_the_limiter_unchanged() {
        let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1_000_000.);
        l.limiter_calculate_offset_ns(0., 100);
        let peeked = l.limiter_peek_offset_ns(10., 100);
        assert_eq!(l.last_update_ns, 0.);
        assert_eq!(l.limiter_calculate_offset_ns(10., 100), peeked);
    }

    #[test]
    fn spacer_first_packet_has_zero_offset() {
        let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1_000_000.);
        assert_eq!(l.limiter_calculate_offset_ns(0., 100), Some(0.));
    }
}
