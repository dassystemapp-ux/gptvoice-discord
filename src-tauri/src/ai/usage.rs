use super::provider::TokenUsage;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};

#[derive(Debug, Default)]
pub struct UsageTracker {
    requests: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl UsageTracker {
    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self, usage: &TokenUsage) {
        self.successes.fetch_add(1, Ordering::Relaxed);
        self.input_tokens
            .fetch_add(usage.input_tokens, Ordering::Relaxed);
        self.output_tokens
            .fetch_add(usage.output_tokens, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            successes: self.successes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
        }
    }
}

static GLOBAL_USAGE: OnceLock<UsageTracker> = OnceLock::new();

pub fn global() -> &'static UsageTracker {
    GLOBAL_USAGE.get_or_init(UsageTracker::default)
}

#[cfg(test)]
mod tests {
    use super::UsageTracker;
    use crate::ai::provider::TokenUsage;

    #[test]
    fn tracks_requests_results_and_tokens() {
        let tracker = UsageTracker::default();
        tracker.record_request();
        tracker.record_success(&TokenUsage {
            input_tokens: 12,
            output_tokens: 8,
            total_tokens: 20,
        });
        tracker.record_request();
        tracker.record_failure();
        assert_eq!(
            tracker.snapshot(),
            super::UsageSnapshot {
                requests: 2,
                successes: 1,
                failures: 1,
                input_tokens: 12,
                output_tokens: 8,
            }
        );
    }
}