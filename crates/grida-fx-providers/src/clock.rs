//! Time for long-job polling (spec/providers.md §7): adapters never sleep except while a
//! `collect` (or a free pre-check inside a `submit`) polls a provider job, and they do it through
//! an injected [`Clock`] so tests poll instantly.

use crate::BoxFuture;
use std::time::{Duration, Instant};

/// Monotonic time and sleeping.
pub trait Clock: Send + Sync {
    /// Time since an arbitrary, fixed origin.
    fn now(&self) -> Duration;
    /// Waits `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()>;
}

/// The real clock: `Instant` and `tokio::time::sleep`.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> SystemClock {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// A clock for tests (feature `testing`): `sleep` returns at once and moves time forward by the
/// duration; `now` returns scripted readings first (each reading once), then the current time.
/// Every sleep is logged.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
pub struct FakeClock {
    state: std::sync::Mutex<FakeState>,
}

#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
struct FakeState {
    now: Duration,
    readings: std::collections::VecDeque<Duration>,
    sleeps: Vec<Duration>,
}

#[cfg(any(test, feature = "testing"))]
impl FakeClock {
    /// Starts at zero.
    pub fn new() -> FakeClock {
        FakeClock::default()
    }

    /// `now` answers these readings first, in order (each sets the current time).
    pub fn scripted(readings: &[Duration]) -> FakeClock {
        let clock = FakeClock::new();
        clock.lock().readings = readings.iter().copied().collect();
        clock
    }

    /// Every sleep so far.
    pub fn sleeps(&self) -> Vec<Duration> {
        self.lock().sleeps.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(any(test, feature = "testing"))]
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        let mut state = self.lock();
        if let Some(reading) = state.readings.pop_front() {
            state.now = reading;
        }
        state.now
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
        let mut state = self.lock();
        state.now += duration;
        state.sleeps.push(duration);
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_clock_moves_on_sleep_and_plays_readings() {
        let clock =
            FakeClock::scripted(&[Duration::ZERO, Duration::ZERO, Duration::from_secs(100)]);
        assert_eq!(clock.now(), Duration::ZERO);
        assert_eq!(clock.now(), Duration::ZERO);
        assert_eq!(clock.now(), Duration::from_secs(100));
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(clock.sleep(Duration::from_secs(5)));
        assert_eq!(clock.now(), Duration::from_secs(105));
        assert_eq!(clock.sleeps(), [Duration::from_secs(5)]);
    }
}
