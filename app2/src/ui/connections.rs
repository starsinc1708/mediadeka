use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui::{Context, Task, Window};
use guise::Button;

use crate::{providers::ProviderId, state::ConnectionMaintenance, storage::Store};

use super::{MainView, workspace::Page};

pub(in crate::ui) struct SourceFailure {
    pub(in crate::ui) message: String,
    pub(in crate::ui) needs_sign_in: bool,
}

impl MainView {
    pub(in crate::ui) fn open_provider_recovery(
        &mut self,
        provider: ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_music_page(Page::Settings, window, cx);
        self.open_settings_connection(provider, window, cx);
    }

    pub(in crate::ui) fn provider_recovery_action(
        &self,
        id: &'static str,
        provider: ProviderId,
        order: isize,
        cx: &Context<Self>,
    ) -> Button {
        let name = match provider {
            ProviderId::YandexMusic => "Яндекс Музыку",
            provider => provider.display_name(),
        };
        Button::new(id, format!("Подключить {name}"))
            .tab_order(order)
            .on_click(cx.listener(move |view, _, window, cx| {
                view.open_provider_recovery(provider, window, cx);
            }))
    }

    pub(in crate::ui) fn connected_provider_profile(&self, provider: ProviderId) -> Option<&str> {
        self.state
            .home_view()?
            .0
            .accounts
            .iter()
            .find(|account| account.provider == provider)
            .map(|account| account.profile.id.as_str())
    }
}

const CHECK_TICK: Duration = Duration::from_secs(30);

pub(super) struct ConnectionWorker {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl ConnectionWorker {
    pub(super) fn start(cx: &mut Context<MainView>) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let running = cancelled.clone();
        let task = cx.spawn(async move |view, cx| {
            let mut maintenance = ConnectionMaintenance::default();
            loop {
                let background = cx.background_executor().clone();
                background.timer(CHECK_TICK).await;
                if running.load(Ordering::Relaxed) {
                    break;
                }
                let flag = running.clone();
                let (next, updates) = background
                    .spawn(async move {
                        let updates =
                            Store::open_default().and_then(|store| maintenance.poll(&store, &flag));
                        (maintenance, updates)
                    })
                    .await;
                maintenance = next;
                if let Ok(updates) = updates
                    && view
                        .update(cx, |view, cx| view.apply_connection_checks(updates, cx))
                        .is_err()
                {
                    break;
                }
            }
        });
        Self {
            cancelled,
            _task: task,
        }
    }
}

impl Drop for ConnectionWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
