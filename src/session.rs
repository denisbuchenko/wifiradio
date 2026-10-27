use std::time::{Duration, Instant};

pub const IDLE_WARNING: Duration = Duration::from_secs(180);
pub const CHIME_MS: u64 = 480;
pub const PEER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct Activity {
    last_voice: Option<Instant>,
}

impl Activity {
    pub fn needs_chime(&self, now: Instant) -> bool {
        self.last_voice
            .is_none_or(|last| now.duration_since(last) >= IDLE_WARNING)
    }

    pub fn voice(&mut self, now: Instant) {
        self.last_voice = Some(now);
    }
}

/// A sliding window accepts reordered UDP packets once, rejecting replayed packets.
#[derive(Debug, Default)]
pub struct ReplayWindow {
    highest: Option<u64>,
    bits: u128,
}

impl ReplayWindow {
    pub fn accept(&mut self, counter: u64) -> bool {
        let Some(highest) = self.highest else {
            self.highest = Some(counter);
            self.bits = 1;
            return true;
        };
        if counter > highest {
            let shift = counter - highest;
            self.bits = if shift >= 128 {
                1
            } else {
                (self.bits << shift) | 1
            };
            self.highest = Some(counter);
            true
        } else {
            let shift = highest - counter;
            if shift >= 128 || self.bits & (1_u128 << shift) != 0 {
                return false;
            }
            self.bits |= 1_u128 << shift;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_on_first_call_and_after_three_minutes_of_silence() {
        let now = Instant::now();
        let mut activity = Activity::default();
        assert!(activity.needs_chime(now));
        activity.voice(now);
        assert!(!activity.needs_chime(now + Duration::from_secs(179)));
        assert!(activity.needs_chime(now + Duration::from_secs(180)));
        activity.voice(now + Duration::from_secs(200));
        assert!(!activity.needs_chime(now + Duration::from_secs(201)));
    }

    #[test]
    fn replay_window_accepts_reordering_but_never_duplicates_or_old_packets() {
        let mut window = ReplayWindow::default();
        assert!(window.accept(12));
        assert!(window.accept(10));
        assert!(!window.accept(10));
        assert!(window.accept(200));
        assert!(!window.accept(12));
        assert!(window.accept(199));
        assert!(!window.accept(200));
    }
}
