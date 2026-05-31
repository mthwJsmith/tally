//! In-process brute-force protection for the authentication endpoints.
//!
//! A per-key failure counter with progressive lockout, held in a single global map.
//! No external dependencies — `std::sync::LazyLock` + `Mutex`. This is sized for a
//! single-instance, self-hosted deployment; if you run multiple replicas, rely on the
//! reverse-proxy / IdP rate limiter in front (see docs/auth-refactor.md) instead, since
//! this state is per-process.
//!
//! Keys are caller-defined and SHOULD include the client IP so that a single abusive
//! source is locked out without locking out the legitimate user from another network.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Failed attempts permitted before lockout begins.
const MAX_FAILURES: u32 = 5;
/// First lockout once the threshold is exceeded; doubles per extra failure up to `LOCK_MAX`.
const LOCK_BASE_SECS: u64 = 30;
const LOCK_MAX_SECS: u64 = 15 * 60;
/// Buckets untouched for this long are pruned.
const IDLE_TTL: Duration = Duration::from_secs(60 * 60);

struct Bucket {
    failures: u32,
    locked_until: Option<Instant>,
    last_seen: Instant,
}

static BUCKETS: LazyLock<Mutex<HashMap<String, Bucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns `Err(retry_after)` if `key` is currently locked out, otherwise `Ok(())`.
/// Call this at the top of a protected handler before doing any credential work.
pub fn check(key: &str) -> Result<(), Duration> {
    let now = Instant::now();
    let map = BUCKETS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(b) = map.get(key) {
        if let Some(until) = b.locked_until {
            if until > now {
                return Err(until - now);
            }
        }
    }
    Ok(())
}

/// Record one failed attempt for `key`; escalates the lockout once over the threshold.
pub fn record_failure(key: &str) {
    let now = Instant::now();
    let mut map = BUCKETS.lock().unwrap_or_else(|p| p.into_inner());
    // Opportunistic prune so the map can't grow without bound.
    map.retain(|_, b| now.duration_since(b.last_seen) < IDLE_TTL);

    let b = map.entry(key.to_string()).or_insert(Bucket {
        failures: 0,
        locked_until: None,
        last_seen: now,
    });
    b.failures = b.failures.saturating_add(1);
    b.last_seen = now;
    if b.failures > MAX_FAILURES {
        // exponential backoff: LOCK_BASE * 2^(over-1), capped at LOCK_MAX.
        let over = (b.failures - MAX_FAILURES).min(20);
        let mult = 1u64.checked_shl(over - 1).unwrap_or(u64::MAX);
        let secs = LOCK_BASE_SECS.saturating_mul(mult).min(LOCK_MAX_SECS);
        b.locked_until = Some(now + Duration::from_secs(secs));
    }
}

/// Clear all state for `key` after a successful authentication.
pub fn record_success(key: &str) {
    let mut map = BUCKETS.lock().unwrap_or_else(|p| p.into_inner());
    map.remove(key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_out_after_threshold_and_clears_on_success() {
        let k = "test:1.2.3.4";
        for _ in 0..MAX_FAILURES {
            assert!(check(k).is_ok());
            record_failure(k);
        }
        // One more failure crosses the threshold and engages lockout.
        record_failure(k);
        assert!(check(k).is_err(), "should be locked out after threshold");
        record_success(k);
        assert!(check(k).is_ok(), "success should clear the lockout");
    }
}
