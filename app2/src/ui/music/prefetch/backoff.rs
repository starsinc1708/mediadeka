use std::time::{Duration, Instant};

use crate::{providers::ProviderId, ui::assets::provider_index};

const MAX_FAILURES: u32 = 5;

#[derive(Default)]
struct Source {
    failures: u32,
    paused_until: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Backoff([Source; ProviderId::ALL.len()]);

impl Backoff {
    pub(super) fn paused_until(&self, provider: ProviderId, now: Instant) -> Option<Instant> {
        self.0[provider_index(provider)]
            .paused_until
            .filter(|until| now < *until)
    }

    pub(super) fn finish(&mut self, provider: ProviderId, ready: bool, network_failed: bool) {
        let source = &mut self.0[provider_index(provider)];
        if ready {
            *source = Source::default();
        } else if network_failed {
            let mut random = [0u8];
            let _ = getrandom::fill(&mut random);
            source.fail(Instant::now(), Duration::from_millis(u64::from(random[0])));
        }
    }
}

impl Source {
    fn fail(&mut self, now: Instant, jitter: Duration) {
        let seconds = 1 << self.failures.min(MAX_FAILURES);
        self.failures = (self.failures + 1).min(MAX_FAILURES);
        self.paused_until = Some(now + Duration::from_secs(seconds) + jitter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_backoff_bounds_delay_to_one_provider_and_success_resets_it() {
        let mut backoff = Backoff::default();
        let now = Instant::now();
        let jitter = Duration::from_millis(80);
        let provider = ProviderId::Jellyfin;
        for seconds in [1, 2, 4, 8, 16, 32, 32] {
            backoff.0[provider_index(provider)].fail(now, jitter);
            let deadline = now + Duration::from_secs(seconds) + jitter;
            assert_eq!(
                backoff.paused_until(provider, deadline - Duration::from_millis(1)),
                Some(deadline)
            );
            assert_eq!(backoff.paused_until(provider, deadline), None);
            assert_eq!(backoff.paused_until(ProviderId::Spotify, now), None);
        }
        backoff.finish(provider, true, false);
        assert_eq!(backoff.paused_until(provider, now), None);
    }
}
