use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use gpui::Task;

use crate::storage::Store;

use super::{
    AccountStatus, Service,
    scrobble::{LastFmAccount, ListenBrainzAccount, Play},
};

const QUEUE_CAPACITY: usize = 16;

#[derive(Clone)]
pub(super) enum Account {
    LastFm(LastFmAccount),
    ListenBrainz(ListenBrainzAccount),
}

impl Account {
    pub(super) fn load(store: &Store, service: Service) -> Result<Option<Self>, String> {
        match service {
            Service::LastFm => store
                .listening_account(service)
                .map(|account| account.map(Self::LastFm)),
            Service::ListenBrainz => store
                .listening_account(service)
                .map(|account| account.map(Self::ListenBrainz)),
        }
        .map_err(|error| error.to_string())
    }

    pub(super) fn save(
        store: &Store,
        service: Service,
        account: Option<&Self>,
    ) -> Result<(), String> {
        match (service, account) {
            (Service::LastFm, Some(Self::LastFm(account))) => {
                store.set_listening_account(service, Some(account))
            }
            (Service::ListenBrainz, Some(Self::ListenBrainz(account))) => {
                store.set_listening_account(service, Some(account))
            }
            (_, None) => store.set_listening_account::<()>(service, None),
            _ => return Err("Подключение относится к другому сервису.".into()),
        }
        .map_err(|error| error.to_string())
    }

    fn name(&self) -> &str {
        match self {
            Self::LastFm(account) => &account.name,
            Self::ListenBrainz(account) => &account.name,
        }
    }

    fn submit(&self, kind: Kind, play: &Play) -> Result<(), String> {
        match (self, kind) {
            (Self::LastFm(account), Kind::NowPlaying) => account.now_playing(play),
            (Self::LastFm(account), Kind::Scrobble) => account.scrobble(play),
            (Self::ListenBrainz(account), Kind::NowPlaying) => account.now_playing(play),
            (Self::ListenBrainz(account), Kind::Scrobble) => account.scrobble(play),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    NowPlaying,
    Scrobble,
}

struct Submission {
    account: Account,
    permit: Arc<AtomicBool>,
    kind: Kind,
    play: Play,
}

pub(super) struct Delivery {
    pub service: Service,
    permit: Arc<AtomicBool>,
    result: Result<(), String>,
}

pub(super) struct Row {
    pub service: Service,
    pub status: AccountStatus,
    pub account: Option<Account>,
    pub cancelled: Option<Arc<AtomicBool>>,
    pub task: Option<Task<()>>,
    permit: Option<Arc<AtomicBool>>,
    worker: Option<async_channel::Sender<Submission>>,
}

impl Row {
    pub(super) fn new(service: Service) -> Self {
        Self {
            service,
            status: AccountStatus::Disconnected,
            account: None,
            cancelled: None,
            task: None,
            permit: None,
            worker: None,
        }
    }

    pub(super) fn connected_status(&self) -> AccountStatus {
        self.account
            .as_ref()
            .map_or(AccountStatus::Disconnected, |account| {
                AccountStatus::Connected(account.name().to_owned())
            })
    }

    pub(super) fn set_account(&mut self, account: Option<Account>) {
        self.set_enabled(false);
        self.account = account;
        self.status = self.connected_status();
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        if enabled && self.account.is_some() {
            self.permit
                .get_or_insert_with(|| Arc::new(AtomicBool::new(true)));
        } else if let Some(permit) = self.permit.take() {
            permit.store(false, Ordering::Release);
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.permit.is_some()
    }

    pub(super) fn dispatch(
        &mut self,
        kind: Kind,
        play: &Play,
        deliveries: &async_channel::Sender<Delivery>,
    ) -> bool {
        let (Some(permit), Some(account)) = (&self.permit, &self.account) else {
            return false;
        };
        let submission = Submission {
            account: account.clone(),
            permit: Arc::clone(permit),
            kind,
            play: play.clone(),
        };
        let worker = if let Some(worker) = &self.worker {
            worker
        } else {
            match start_worker(self.service, deliveries.clone()) {
                Ok(worker) => self.worker.insert(worker),
                Err(error) => {
                    let _ = deliveries.try_send(Delivery {
                        service: self.service,
                        permit: Arc::clone(permit),
                        result: Err(error),
                    });
                    return false;
                }
            }
        };
        if worker.try_send(submission).is_err() {
            let _ = deliveries.try_send(Delivery {
                service: self.service,
                permit: Arc::clone(permit),
                result: Err("Не удалось поставить прослушивание в очередь отправки.".into()),
            });
            return false;
        }
        true
    }

    pub(super) fn delivered(&mut self, delivery: Delivery) -> bool {
        if self.task.is_some()
            || self
                .permit
                .as_ref()
                .is_none_or(|permit| !Arc::ptr_eq(permit, &delivery.permit))
        {
            return false;
        }
        let status = match delivery.result {
            Ok(()) => self.connected_status(),
            Err(error) => AccountStatus::Failed(error),
        };
        let changed = self.status != status;
        self.status = status;
        changed
    }
}

impl Drop for Row {
    fn drop(&mut self) {
        self.set_enabled(false);
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
    }
}

fn start_worker(
    service: Service,
    deliveries: async_channel::Sender<Delivery>,
) -> Result<async_channel::Sender<Submission>, String> {
    let (worker, queued) = async_channel::bounded::<Submission>(QUEUE_CAPACITY);
    thread::Builder::new()
        .name(format!("mediadeka-{}", service.id()))
        .spawn(move || {
            while let Ok(submission) = queued.recv_blocking() {
                if !submission.permit.load(Ordering::Acquire) {
                    continue;
                }
                let result = submission.account.submit(submission.kind, &submission.play);
                if deliveries
                    .try_send(Delivery {
                        service,
                        permit: submission.permit,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .map_err(|_| "Не удалось запустить очередь отправки прослушиваний.".to_owned())?;
    Ok(worker)
}
