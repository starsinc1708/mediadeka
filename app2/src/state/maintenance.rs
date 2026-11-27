use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::{
    providers::{self, ProviderError, ProviderId},
    storage::{Account, Store, StoreError},
};

use super::{
    Issue,
    recovery::{RENEWAL_LEAD, restore_connection},
};

const HEALTH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(crate) struct ConnectionMaintenance {
    accounts: BTreeMap<ProviderId, Scheduled>,
}

struct Scheduled {
    account: Account,
    next: Option<Instant>,
    failures: u32,
    terminal: Option<ProviderError>,
}

pub(crate) struct ConnectionCheck {
    pub(crate) account: Account,
    pub(crate) problem: Option<ProviderError>,
}

impl ConnectionMaintenance {
    pub(crate) fn poll(
        &mut self,
        store: &Store,
        cancelled: &AtomicBool,
    ) -> Result<Vec<ConnectionCheck>, StoreError> {
        self.poll_with(store, cancelled, |store, account, cancelled| {
            restore_connection(store, account, cancelled, true)
                .map(|session| session.credentials.renewal_deadline())
        })
    }

    fn poll_with(
        &mut self,
        store: &Store,
        cancelled: &AtomicBool,
        mut check: impl FnMut(&Store, &Account, &AtomicBool) -> Result<Option<u64>, Issue>,
    ) -> Result<Vec<ConnectionCheck>, StoreError> {
        let mut updates = Vec::new();
        for provider in ProviderId::ALL {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let Some(account) = store.preferred_account(provider)? else {
                self.accounts.remove(&provider);
                continue;
            };
            let now = Instant::now();
            let scheduled = self
                .accounts
                .entry(provider)
                .or_insert_with(|| Scheduled::new(account.clone(), now));
            if scheduled.account != account {
                *scheduled = Scheduled::new(account.clone(), now);
            }
            if scheduled.next.is_none_or(|next| next > now) {
                if let Some(problem) = &scheduled.terminal {
                    updates.push(ConnectionCheck {
                        account,
                        problem: Some(problem.clone()),
                    });
                }
                continue;
            }

            let result = check(store, &account, cancelled);
            let now = Instant::now();
            match result {
                Ok(expiry) => {
                    let Some(current) = store.account(provider, &account.profile.id)? else {
                        continue;
                    };
                    *scheduled = Scheduled::new(
                        current.clone(),
                        now + normal_delay(expiry, providers::epoch_seconds()),
                    );
                    updates.push(ConnectionCheck {
                        account: current,
                        problem: None,
                    });
                }
                Err(Issue::Provider(error)) if error.needs_sign_in() => {
                    if store.account(provider, &account.profile.id)?.as_ref() == Some(&account) {
                        scheduled.next = None;
                        scheduled.terminal = Some(error.clone());
                        updates.push(ConnectionCheck {
                            account,
                            problem: Some(error),
                        });
                    }
                }
                Err(_) => {
                    scheduled.next = Some(now + retry_delay(scheduled.failures));
                    scheduled.failures = scheduled.failures.saturating_add(1);
                }
            }
        }
        Ok(updates)
    }
}

impl Scheduled {
    fn new(account: Account, next: Instant) -> Self {
        Self {
            account,
            next: Some(next),
            failures: 0,
            terminal: None,
        }
    }
}

fn normal_delay(expiry: Option<u64>, epoch: Option<u64>) -> Duration {
    expiry.zip(epoch).map_or(HEALTH_INTERVAL, |(expiry, now)| {
        let remaining = expiry.saturating_sub(now);
        let until_window = remaining.saturating_sub(RENEWAL_LEAD.as_secs());
        let delay = if until_window == 0 {
            remaining / 2
        } else {
            until_window
        };
        Duration::from_secs(delay).clamp(RETRY_INTERVAL, HEALTH_INTERVAL)
    })
}

fn retry_delay(failures: u32) -> Duration {
    (RETRY_INTERVAL * (1 << failures.min(4))).min(MAX_RETRY)
}

#[cfg(all(test, windows))]
mod tests;
