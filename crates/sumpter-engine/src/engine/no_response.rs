//! Short-lived protection for an identical request repeatedly failing before headers.
//! Only digests and counters are retained; requests, credentials and failures are not stored.

use std::collections::HashMap;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sumpter_core::request_policy::RequestPolicy;
use tokio::time::Instant;

pub(super) type Fingerprint = [u8; 32];
const MAX_ENTRIES: usize = 2048;
const FAILURE_RETENTION: Duration = Duration::from_secs(30 * 60);

/// Length delimiters prevent ambiguous concatenations of identity components.
pub(super) fn fingerprint(parts: &[&[u8]]) -> Fingerprint {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}

#[derive(Clone, Copy)]
pub(super) struct BlockedRequest {
    pub(super) until: Instant,
}

impl BlockedRequest {
    pub(super) fn remaining_seconds(self) -> u64 {
        self.until
            .saturating_duration_since(Instant::now())
            .as_secs_f64()
            .ceil() as u64
    }
}

struct Entry {
    failures: u32,
    updated_at: Instant,
    blocked: Option<BlockedRequest>,
}

impl Entry {
    fn live(&self, now: Instant) -> bool {
        self.blocked.map_or_else(
            || now.saturating_duration_since(self.updated_at) < FAILURE_RETENTION,
            |blocked| now < blocked.until,
        )
    }
}

#[derive(Default)]
pub(super) struct NoResponseGuard {
    entries: HashMap<Fingerprint, Entry>,
}

impl NoResponseGuard {
    pub(super) fn blocked(&mut self, key: &Fingerprint) -> Option<BlockedRequest> {
        let now = Instant::now();
        let entry = self.entries.get(key)?;
        if !entry.live(now) {
            self.entries.remove(key);
            return None;
        }
        entry.blocked
    }

    pub(super) fn note_failure(
        &mut self,
        key: Fingerprint,
        policy: &RequestPolicy,
        request_failures: u32,
    ) -> Option<BlockedRequest> {
        if policy.max_consecutive_no_response == 0 {
            return None;
        }
        // Concurrent failures and rejected resends must not slide an open deadline.
        if let Some(blocked) = self.blocked(&key) {
            return Some(blocked);
        }
        let now = Instant::now();
        if !self.entries.contains_key(&key) {
            self.entries.retain(|_, entry| entry.live(now));
            if self.entries.len() >= MAX_ENTRIES
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.updated_at)
                    .map(|(key, _)| *key)
            {
                self.entries.remove(&oldest);
            }
        }
        let entry = self.entries.entry(key).or_insert(Entry {
            failures: 0,
            updated_at: now,
            blocked: None,
        });
        // The admitted request keeps its own streak even if a very slow attempt
        // outlives retention, or other traffic evicts its cross-request entry.
        entry.failures = entry.failures.saturating_add(1).max(request_failures);
        entry.updated_at = now;
        if entry.failures >= policy.max_consecutive_no_response {
            entry.blocked = Some(BlockedRequest {
                until: now + Duration::from_secs(u64::from(policy.no_response_cooldown_seconds)),
            });
        }
        entry.blocked
    }

    /// Any actual HTTP response breaks a *pre-header* failure streak, even an HTTP error.
    pub(super) fn note_response(&mut self, key: &Fingerprint) {
        self.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn consecutive_failures_expire_and_blocked_resends_do_not_extend_cooldown() {
        let mut guard = NoResponseGuard::default();
        let policy = RequestPolicy::default();
        let key = fingerprint(&[b"session", b"request", b"endpoint"]);
        assert!(guard.note_failure(key, &policy, 1).is_none());
        tokio::time::advance(FAILURE_RETENTION).await;
        assert!(guard.note_failure(key, &policy, 1).is_none());
        assert!(guard.note_failure(key, &policy, 1).is_none());
        let blocked = guard.note_failure(key, &policy, 1).unwrap();
        assert_eq!(blocked.remaining_seconds(), 300);
        tokio::time::advance(Duration::from_secs(299)).await;
        assert_eq!(
            guard.note_failure(key, &policy, 1).unwrap().until,
            blocked.until
        );
        assert_eq!(guard.blocked(&key).unwrap().remaining_seconds(), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(guard.blocked(&key).is_none());
        assert!(guard.note_failure(key, &policy, 1).is_none());
        tokio::time::advance(FAILURE_RETENTION).await;
        assert!(
            guard.note_failure(key, &policy, 3).is_some(),
            "an admitted request retains its own bound"
        );
    }

    #[test]
    fn responses_reset_streaks_and_disabled_protection_records_nothing() {
        let mut guard = NoResponseGuard::default();
        let mut policy = RequestPolicy::default();
        let key = fingerprint(&[b"request"]);
        for _ in 0..2 {
            assert!(guard.note_failure(key, &policy, 1).is_none());
        }
        guard.note_response(&key);
        assert!(guard.note_failure(key, &policy, 1).is_none());
        policy.max_consecutive_no_response = 0;
        let other = fingerprint(&[b"other"]);
        for _ in 0..10 {
            assert!(guard.note_failure(other, &policy, 1).is_none());
        }
        assert!(!guard.entries.contains_key(&other));
    }

    #[test]
    fn fingerprints_are_delimited_and_storage_is_bounded() {
        assert_ne!(fingerprint(&[b"a", b"bc"]), fingerprint(&[b"ab", b"c"]));
        let mut guard = NoResponseGuard::default();
        for i in 0..MAX_ENTRIES + 20 {
            guard.note_failure(
                fingerprint(&[&i.to_be_bytes()]),
                &RequestPolicy::default(),
                1,
            );
        }
        assert_eq!(guard.entries.len(), MAX_ENTRIES);
    }
}
