//! Who else has the GUI open: every `GET /api/online` poll refreshes its session id; a session
//! that has not polled for `CLIENT_TIMEOUT_SECS` is forgotten.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A session counts while its last poll is at most this old (inclusive).
pub const CLIENT_TIMEOUT_SECS: u64 = 15;

/// Session ids are cut to this many bytes (on a char boundary).
pub const MAX_ID_BYTES: usize = 64;

#[derive(Default)]
pub struct SessionTracker {
    seen: Mutex<HashMap<String, Instant>>,
}

impl SessionTracker {
    /// Records a poll of `id` and returns the number of OTHER live sessions.
    pub fn track_and_count_others(&self, id: &str) -> usize {
        self.track_at(id, Instant::now())
    }

    fn track_at(&self, id: &str, now: Instant) -> usize {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.insert(truncate(id).to_owned(), now);
        let timeout = Duration::from_secs(CLIENT_TIMEOUT_SECS);
        seen.retain(|_, last| now.saturating_duration_since(*last) <= timeout);
        seen.len().saturating_sub(1)
    }
}

/// `id` cut to at most `MAX_ID_BYTES` bytes without splitting a character.
fn truncate(id: &str) -> &str {
    let mut end = id.len().min(MAX_ID_BYTES);
    while !id.is_char_boundary(end) {
        end -= 1;
    }
    &id[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_counts_up_to_exactly_the_timeout_and_is_gone_after_it() {
        let t = SessionTracker::default();
        let t0 = Instant::now();
        assert_eq!(t.track_at("a", t0), 0);
        let at_limit = t0 + Duration::from_secs(CLIENT_TIMEOUT_SECS);
        assert_eq!(t.track_at("b", at_limit), 1, "15 s old still counts (<=)");
        let past = at_limit + Duration::from_millis(1);
        assert_eq!(t.track_at("b", past), 0, "older than 15 s is forgotten");
    }

    #[test]
    fn a_refreshed_session_is_one_session() {
        let t = SessionTracker::default();
        let t0 = Instant::now();
        t.track_at("a", t0);
        assert_eq!(t.track_at("a", t0 + Duration::from_secs(5)), 0);
    }
}
