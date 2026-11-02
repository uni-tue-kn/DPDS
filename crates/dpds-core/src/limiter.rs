use serde::{Deserialize, Serialize};

const L1_OVERHEAD: u32 = 7 + 1 + 12; // Preamble + SFD + IPG

#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum LimiterMode {
    Spacer,
    TokenBucketFilter,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Limiter {
    pub mode: LimiterMode,
    pub activate: bool,      // true if limiter should be applied
    pub rate_bpns: f64,      // in bits per nanosecond
    pub last_update_ns: f64, // in nanoseconds
    pub limit_bit: f64,      // in bits
    pub fill_state_bit: f64, // in bits (fill state for Spacer, contingent for TBF)
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
    pub fn limiter_calculate_offset_ns(
        &mut self,
        time_to_sent_ns: f64,
        packet_length_byte: u32,
    ) -> Option<f64> {
        if !self.activate {
            return Some(0.);
        }

        // Calculate time between last update and time to send
        // The duration is dependent on the std delay
        // > The higher the std delay, the wider the time to sent is spread and the bigger the duration
        // >> Therefore, the limit has to be increased dependent on the std
        let duration_since_last_update_ns = (time_to_sent_ns - self.last_update_ns).max(0.);

        // Update time to send if time to send is later than the old last update
        self.last_update_ns = f64::max(self.last_update_ns, time_to_sent_ns);

        // Calculate packet size
        // Packets do not include L1 overhead
        let packet_size_in_bit = ((packet_length_byte + L1_OVERHEAD) * 8) as f64;

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

    /// Spacer: schedules packets by calculating a send offset based on the fill state.
    fn spacer_calculate_offset_ns(
        &mut self,
        time_to_sent_ns: f64,
        duration_since_last_update_ns: f64,
        packet_size_in_bit: f64,
    ) -> Option<f64> {
        // Reduce fill state by the amount of bits that can be sent since last update of fill state until the time to send
        self.fill_state_bit = f64::max(
            0.,
            self.fill_state_bit - duration_since_last_update_ns * self.rate_bpns,
        );

        if self.fill_state_bit + packet_size_in_bit > self.limit_bit {
            None
        } else {
            let offset_between_time_to_sent_and_last_update =
                (self.last_update_ns - time_to_sent_ns).max(0.);

            // Calculate when the remaining fill state (without this packet) will be sent
            // Add the duration between time_to_sent and last_update
            // -> Else the offset would be relative to time_to_sent and not last_update
            let time_offset_ns = (self.fill_state_bit / self.rate_bpns)
                + offset_between_time_to_sent_and_last_update;

            // Update fill state after calculating offset
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
        // Refill tokens: add tokens proportional to elapsed time, capped at limit
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
    fn spacer_first_packet_has_zero_offset() {
        let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1_000_000.);
        assert_eq!(l.limiter_calculate_offset_ns(0., 100), Some(0.));
    }
}
