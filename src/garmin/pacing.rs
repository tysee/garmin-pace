//! src/garmin/pacing.rs paces every request to each Garmin host across this process.
//! Requests are spaced by garmin.request_interval_ms; after HTTP 429 the spacing doubles for
//! the rest of the process, up to ADAPTIVE_FACTOR times the configured interval. Waits are
//! reported to one observer (the terminal menu) or to stderr; `observe` is the public API.

use anyhow::{Result, anyhow};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// After repeated rate limits, spacing grows to at most this multiple of the configured interval.
const ADAPTIVE_FACTOR: u32 = 8;

/// A pause Garmin asked for before the same read is attempted again.
pub struct RateWait {
    pub seconds: u64,
    pub attempt: u32,
    pub attempts: u32,
}

/// Receives rate-limit waits; a failed report stops the request rather than hiding the wait.
pub type WaitObserver = Box<dyn Fn(&RateWait) -> std::io::Result<()> + Send + Sync>;

struct Host {
    interval: Duration,
    next: Instant,
}

#[derive(Default)]
struct State {
    hosts: HashMap<String, Host>,
    observer: Option<WaitObserver>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Result<T> {
    let mut guard = STATE
        .lock()
        .map_err(|_| anyhow!("Garmin request pacing state is unavailable"))?;
    Ok(f(guard.get_or_insert_with(State::default)))
}

/// Route rate-limit waits to an observer, or back to stderr with None.
pub fn observe(observer: Option<WaitObserver>) -> Result<()> {
    with_state(|state| state.observer = observer)
}

/// Reserve the next request slot for this host and sleep until it starts.
pub(super) fn before_request(host: &str, configured: Duration) -> Result<()> {
    let now = Instant::now();
    let start = with_state(|state| {
        let entry = state.hosts.entry(host.into()).or_insert(Host {
            interval: configured,
            next: now,
        });
        entry.interval = entry.interval.max(configured);
        let start = entry.next.max(now);
        entry.next = start + entry.interval;
        start
    })?;
    std::thread::sleep(start.saturating_duration_since(now));
    Ok(())
}

/// Space later requests to this host further apart after Garmin answered HTTP 429.
pub(super) fn slow_down(host: &str, configured: Duration) -> Result<()> {
    with_state(|state| {
        if let Some(entry) = state.hosts.get_mut(host) {
            entry.interval = (entry.interval * 2).min(configured * ADAPTIVE_FACTOR);
        }
    })
}

/// Report a wait, then pause this thread for it.
pub(super) fn wait(pause: &RateWait) -> Result<()> {
    with_state(|state| match &state.observer {
        Some(observer) => observer(pause),
        None => {
            eprintln!(
                "warning: Garmin rate limit; waiting {} s before retry {}/{} and slowing later requests",
                pause.seconds, pause.attempt, pause.attempts
            );
            Ok(())
        }
    })?
    .map_err(|error| anyhow!("reporting Garmin rate-limit wait failed: {error}"))?;
    std::thread::sleep(Duration::from_secs(pause.seconds));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{before_request, slow_down, with_state};
    use std::time::{Duration, Instant};

    #[test]
    fn requests_are_spaced_and_rate_limits_double_the_spacing() -> anyhow::Result<()> {
        let host = "pacing-test-host";
        let interval = Duration::from_millis(20);
        let started = Instant::now();
        before_request(host, interval)?;
        before_request(host, interval)?;
        assert!(started.elapsed() >= interval);
        slow_down(host, interval)?;
        let doubled = with_state(|state| state.hosts.get(host).map(|h| h.interval))?;
        assert_eq!(doubled, Some(interval * 2));
        for _ in 0..10 {
            slow_down(host, interval)?;
        }
        let capped = with_state(|state| state.hosts.get(host).map(|h| h.interval))?;
        assert_eq!(capped, Some(interval * 8));
        Ok(())
    }
}
