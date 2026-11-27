mod discord;
mod playback;
mod scrobble;
mod submission;

#[cfg(all(test, windows))]
mod tests;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, Task};
use serde::{Deserialize, Serialize};

use crate::{
    playback::{ListeningSample, SoundStarted},
    storage::Store,
};

use playback::Listening;
use submission::{Account, Delivery, Row};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct Preferences {
    pub discord_enabled: bool,
    pub discord_paused: bool,
    pub discord_private: bool,
    pub discord_application_id: String,
    pub lastfm_enabled: bool,
    pub listenbrainz_enabled: bool,
}

impl Preferences {
    fn enabled(&self, service: Service) -> bool {
        match service {
            Service::LastFm => self.lastfm_enabled,
            Service::ListenBrainz => self.listenbrainz_enabled,
        }
    }

    fn validate(&mut self) -> Result<(), String> {
        let id = self.discord_application_id.trim();
        if (self.discord_enabled || !id.is_empty())
            && (!id.bytes().all(|byte| byte.is_ascii_digit())
                || !id.parse::<u64>().is_ok_and(|value| value != 0))
        {
            return Err("Укажите числовой ID приложения Discord.".into());
        }
        self.discord_application_id = id.to_owned();
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Service {
    LastFm,
    ListenBrainz,
}

impl Service {
    pub(crate) const ALL: [Self; 2] = [Self::LastFm, Self::ListenBrainz];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::LastFm => "lastfm",
            Self::ListenBrainz => "listenbrainz",
        }
    }
}

#[derive(Eq, PartialEq)]
pub(crate) enum AccountStatus {
    Loading,
    Disconnected,
    Connecting,
    Connected(String),
    Failed(String),
}

pub(crate) struct Integrations {
    preferences: Preferences,
    preferences_error: Option<String>,
    preferences_task: Option<Task<()>>,
    initialized: bool,
    rows: [Row; Service::ALL.len()],
    listening: Listening,
    deliveries: async_channel::Sender<Delivery>,
    _delivery_task: Task<()>,
}

impl Integrations {
    pub(crate) fn new(load_profile: bool, cx: &mut Context<Self>) -> Self {
        let (deliveries, results) = async_channel::unbounded();
        let _delivery_task = cx.spawn(async move |this, cx| {
            while let Ok(delivery) = results.recv().await {
                if this
                    .update(cx, |this, cx| this.delivered(delivery, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            preferences: Preferences::default(),
            preferences_error: None,
            preferences_task: None,
            initialized: !load_profile,
            rows: Service::ALL.map(Row::new),
            listening: Listening::default(),
            deliveries,
            _delivery_task,
        };
        if load_profile {
            this.load(cx);
        }
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.initialized = false;
        for row in &mut self.rows {
            row.status = AccountStatus::Loading;
        }
        self.preferences_task = Some(cx.spawn(async move |this, cx| {
            let (preferences, accounts) = cx
                .background_executor()
                .spawn(async {
                    let store = Store::open_default();
                    let preferences =
                        store
                            .as_ref()
                            .map_err(ToString::to_string)
                            .and_then(|store| {
                                let mut preferences = store
                                    .listening_preferences()
                                    .map_err(|error| error.to_string())?;
                                preferences.validate()?;
                                Ok(preferences)
                            });
                    let accounts = Service::ALL.map(|service| {
                        store
                            .as_ref()
                            .map_err(ToString::to_string)
                            .and_then(|store| Account::load(store, service))
                    });
                    (preferences, accounts)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.preferences_task = None;
                this.initialized = true;
                match preferences {
                    Ok(preferences) => this.preferences = preferences,
                    Err(error) => this.preferences_error = Some(error),
                }
                for (row, account) in this.rows.iter_mut().zip(accounts) {
                    match account {
                        Ok(account) => row.set_account(account),
                        Err(error) => row.status = AccountStatus::Failed(error),
                    }
                }
                this.apply_preferences();
                cx.notify();
            });
        }));
    }

    #[cfg(feature = "capture")]
    pub(crate) fn load_for_capture(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    pub(crate) fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    pub(crate) fn preferences_busy(&self) -> bool {
        self.preferences_task.is_some()
    }

    pub(crate) fn preferences_error(&self) -> Option<&str> {
        self.preferences_error.as_deref()
    }

    pub(crate) fn set_preferences(&mut self, mut preferences: Preferences, cx: &mut Context<Self>) {
        if self.preferences_busy() {
            return;
        }
        self.preferences_error = preferences.validate().err();
        if self.preferences_error.is_some() || self.preferences == preferences {
            cx.notify();
            return;
        }
        self.preferences_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Store::open_default()?.set_listening_preferences(&preferences)?;
                    Ok::<_, crate::storage::StoreError>(preferences)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.preferences_task = None;
                match result {
                    Ok(saved) => {
                        this.preferences = saved;
                        this.apply_preferences();
                    }
                    Err(error) => this.preferences_error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn account(&self, service: Service) -> &AccountStatus {
        &self.rows[service as usize].status
    }

    pub(crate) fn is_connected(&self, service: Service) -> bool {
        self.rows[service as usize].account.is_some()
    }

    pub(crate) fn connect_lastfm(&mut self, key: String, secret: String, cx: &mut Context<Self>) {
        let (opener, urls) = async_channel::bounded::<String>(1);
        let browser = cx.spawn(async move |_, cx| {
            if let Ok(url) = urls.recv().await {
                cx.update(|cx| cx.open_url(&url));
            }
        });
        self.connect(
            Service::LastFm,
            cx,
            move |cancelled| {
                scrobble::last_fm_connect(key, secret, cancelled, |url| {
                    opener
                        .try_send(url.to_owned())
                        .map_err(|_| "Не удалось открыть подтверждение Last.fm.".into())
                })
                .map(Account::LastFm)
            },
            Some(browser),
        );
    }

    pub(crate) fn connect_listenbrainz(&mut self, token: String, cx: &mut Context<Self>) {
        self.connect(
            Service::ListenBrainz,
            cx,
            move |_| scrobble::listenbrainz_connect(token).map(Account::ListenBrainz),
            None,
        );
    }

    fn connect(
        &mut self,
        service: Service,
        cx: &mut Context<Self>,
        authorize: impl FnOnce(&AtomicBool) -> Result<Account, String> + Send + 'static,
        browser: Option<Task<()>>,
    ) {
        let row = &mut self.rows[service as usize];
        if !self.initialized || row.task.is_some() {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        row.cancelled = Some(Arc::clone(&cancelled));
        row.status = AccountStatus::Connecting;
        row.task = Some(cx.spawn(async move |this, cx| {
            let worker_flag = Arc::clone(&cancelled);
            let authorized = cx
                .background_executor()
                .spawn(async move { authorize(&worker_flag) })
                .await;
            drop(browser);
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let row = &mut this.rows[service as usize];
                row.cancelled = None;
                row.task = None;
                match authorized {
                    Ok(account) => this.save_account(service, Some(account), cx),
                    Err(error) => {
                        row.status = AccountStatus::Failed(error);
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    pub(crate) fn cancel(&mut self, service: Service, cx: &mut Context<Self>) {
        let row = &mut self.rows[service as usize];
        if let Some(flag) = row.cancelled.take() {
            flag.store(true, Ordering::Release);
            row.task = None;
            row.status = row.connected_status();
            cx.notify();
        }
    }

    pub(crate) fn disconnect(&mut self, service: Service, cx: &mut Context<Self>) {
        self.save_account(service, None, cx);
    }

    fn save_account(&mut self, service: Service, account: Option<Account>, cx: &mut Context<Self>) {
        let row = &mut self.rows[service as usize];
        if !self.initialized || row.task.is_some() {
            return;
        }
        row.set_enabled(false);
        row.status = AccountStatus::Loading;
        row.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default().map_err(|error| error.to_string())?;
                    Account::save(&store, service, account.as_ref())?;
                    Ok::<_, String>(account)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let row = &mut this.rows[service as usize];
                row.task = None;
                match result {
                    Ok(account) => row.set_account(account),
                    Err(error) => row.status = AccountStatus::Failed(error),
                }
                this.apply_preferences();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn apply_preferences(&mut self) {
        for row in &mut self.rows {
            let enabled =
                self.preferences.enabled(row.service) && row.status != AccountStatus::Loading;
            row.set_enabled(enabled);
        }
        self.reconcile_playback();
    }

    fn delivered(&mut self, delivery: Delivery, cx: &mut Context<Self>) {
        let row = &mut self.rows[delivery.service as usize];
        if row.delivered(delivery) {
            cx.notify();
        }
    }

    pub(crate) fn started(&mut self, sound: &SoundStarted) {
        self.listening.started(sound);
    }

    pub(crate) fn playback_changed(&mut self, sample: Option<ListeningSample>) {
        self.listening.sample(sample);
        self.reconcile_playback();
    }

    fn reconcile_playback(&mut self) {
        self.listening
            .reconcile(&self.preferences, &mut self.rows, &self.deliveries);
    }
}
