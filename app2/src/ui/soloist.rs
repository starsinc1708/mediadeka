#[cfg(feature = "capture")]
mod acceptance;
mod view;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{AppContext, Context, Entity, FocusHandle, Subscription, Task, Window};
use guise::{TextInput, TextInputEvent};

use crate::{
    playlist::PlaylistKey,
    providers::ProviderId,
    soloist::{self, PairProgress, Readiness},
};

use super::{MainView, player::Snapshot, workspace::Page};

pub(super) struct SoloistUi {
    readiness: Option<Readiness>,
    issue: Option<String>,
    dialog: Option<Dialog>,
    operation: Option<Operation>,
    next_id: u64,
    key: Entity<TextInput>,
    focus: FocusHandle,
    close_focus: FocusHandle,
    settings_focus: FocusHandle,
    focus_pending: Option<ModalFocus>,
    _lifetime: soloist::Lifetime,
    _submit_subscription: Subscription,
}

struct Dialog {
    opened_for: Option<Snapshot>,
    restore: Option<FocusHandle>,
}

enum ModalFocus {
    Close,
    Key,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Check,
    Install,
    Pair,
}

struct Operation {
    id: u64,
    kind: Kind,
    cancelled: Arc<AtomicBool>,
    progress: Option<PairProgress>,
    _task: Task<()>,
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

enum Work {
    Check { account: Option<String> },
    Install { account: Option<String> },
    Pair { account: String, key: String },
}

enum Event {
    Progress(PairProgress),
    Finished(Result<Readiness, String>),
}

impl Work {
    fn kind(&self) -> Kind {
        match self {
            Self::Check { .. } => Kind::Check,
            Self::Install { .. } => Kind::Install,
            Self::Pair { .. } => Kind::Pair,
        }
    }

    fn run(
        self,
        cancelled: &AtomicBool,
        progress: impl Fn(PairProgress),
    ) -> Result<Readiness, String> {
        match self {
            Self::Check { account } => soloist::refresh(account.as_deref(), cancelled),
            Self::Install { account } => {
                soloist::install_for_profile(account.as_deref(), cancelled)
            }
            Self::Pair { account, key } => {
                soloist::pair_for_profile(&account, key, cancelled, progress)
            }
        }
    }
}

impl SoloistUi {
    pub(super) fn new(window: &Window, cx: &mut Context<MainView>, manage_process: bool) -> Self {
        let key = cx.new(|cx| {
            TextInput::new(cx)
                .id("soloist-key")
                .placeholder("Ключ Spotify Soloist")
                .password(true)
                .tab_order(710)
        });
        let submit_subscription = cx.subscribe_in(
            &key,
            window,
            |view, _, event: &TextInputEvent, window, cx| {
                if matches!(event, TextInputEvent::Submit(_))
                    && view.soloist.active().is_none()
                    && matches!(view.soloist.readiness, Some(Readiness::Installed { .. }))
                {
                    view.submit_soloist(window, cx);
                }
            },
        );
        Self {
            readiness: None,
            issue: None,
            dialog: None,
            operation: None,
            next_id: 0,
            key,
            focus: cx.focus_handle(),
            close_focus: cx.focus_handle().tab_index(708).tab_stop(true),
            settings_focus: cx.focus_handle().tab_index(40).tab_stop(true),
            focus_pending: None,
            _lifetime: soloist::Lifetime::new(manage_process),
            _submit_subscription: submit_subscription,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(super) fn modal_focus(&self) -> Option<&FocusHandle> {
        self.dialog.as_ref().map(|_| &self.focus)
    }

    fn active(&self) -> Option<Kind> {
        self.operation.as_ref().map(|operation| operation.kind)
    }
}

impl MainView {
    pub(super) fn start_soloist_on_startup(&mut self, cx: &mut Context<Self>) {
        self.run_soloist(Work::Check { account: None }, cx);
    }

    fn spotify_account(&self) -> Option<String> {
        self.state.home_view().and_then(|(home, _)| {
            home.accounts
                .iter()
                .find(|account| account.provider == ProviderId::Spotify)
                .map(|account| account.profile.id.clone())
        })
    }

    pub(super) fn open_soloist_on_playback(
        &mut self,
        state: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_settings_login() {
            let _ = self
                .player
                .update(cx, |bar, cx| bar.dismiss_devices(window, cx));
            self.open_soloist(Some(state), window, cx);
        }
    }

    fn open_soloist(
        &mut self,
        playback: Option<Snapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.soloist.dialog.as_mut() {
            if playback.is_some() {
                dialog.opened_for = playback;
            }
            return;
        }
        let account = playback
            .as_ref()
            .and_then(|state| match &state.key {
                Some(PlaylistKey::Source {
                    provider: ProviderId::Spotify,
                    profile,
                    ..
                }) => profile.clone(),
                _ => None,
            })
            .or_else(|| self.spotify_account());
        self.stop_workspace_scrolling();
        self.soloist.dialog = Some(Dialog {
            opened_for: playback,
            restore: window.focused(cx),
        });
        self.soloist.focus_pending = Some(ModalFocus::Close);
        if self.soloist.operation.is_none() {
            self.run_soloist(Work::Check { account }, cx);
        }
        cx.notify();
    }

    pub(super) fn close_soloist(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.soloist.dialog.take() else {
            return false;
        };
        if matches!(self.soloist.active(), Some(Kind::Pair | Kind::Install)) {
            self.soloist.operation.take();
        }
        self.soloist.focus_pending = None;
        self.soloist
            .key
            .update(cx, |field, cx| field.set_text("", cx));
        window.focus(dialog.restore.as_ref().unwrap_or(&self.focus), cx);
        cx.notify();
        true
    }

    pub(super) fn focus_soloist_if_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.soloist.dialog.is_some() {
            match self.soloist.focus_pending.take() {
                Some(ModalFocus::Close) => window.focus(&self.soloist.close_focus, cx),
                Some(ModalFocus::Key) => self
                    .soloist
                    .key
                    .update(cx, |field, cx| field.focus(window, cx)),
                None => {}
            }
        }
    }

    fn run_soloist(&mut self, work: Work, cx: &mut Context<Self>) {
        self.soloist.operation.take();
        self.soloist.issue = None;
        self.soloist.next_id = self.soloist.next_id.wrapping_add(1);
        let id = self.soloist.next_id;
        let kind = work.kind();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let (tx, rx) = async_channel::unbounded();
        let task = cx.spawn(async move |view, cx| {
            cx.background_executor()
                .spawn(async move {
                    let result = work.run(&cancellation, |progress| {
                        let _ = tx.try_send(Event::Progress(progress));
                    });
                    let _ = tx.try_send(Event::Finished(result));
                })
                .detach();
            while let Ok(event) = rx.recv().await {
                if !view
                    .update(cx, |view, cx| view.soloist_event(id, event, cx))
                    .unwrap_or(false)
                {
                    break;
                }
            }
        });
        self.soloist.operation = Some(Operation {
            id,
            kind,
            cancelled,
            progress: None,
            _task: task,
        });
        cx.notify();
    }

    fn soloist_event(&mut self, id: u64, event: Event, cx: &mut Context<Self>) -> bool {
        let Some(operation) = self
            .soloist
            .operation
            .as_mut()
            .filter(|operation| operation.id == id)
        else {
            return false;
        };
        match event {
            Event::Progress(progress) => {
                operation.progress = Some(progress);
                cx.notify();
                true
            }
            Event::Finished(result) => {
                let kind = operation.kind;
                self.soloist.operation.take();
                match result {
                    Ok(readiness) => {
                        if self.soloist.is_open()
                            && self.spotify_account().is_some()
                            && matches!(readiness, Readiness::Installed { .. })
                        {
                            self.soloist.focus_pending = Some(ModalFocus::Key);
                        }
                        self.soloist.readiness = Some(readiness);
                    }
                    Err(error) => {
                        if kind == Kind::Check {
                            self.soloist.readiness = None;
                        }
                        if kind == Kind::Pair && self.soloist.is_open() {
                            self.soloist.focus_pending = Some(ModalFocus::Key);
                        }
                        self.soloist.issue = Some(error);
                    }
                }
                cx.notify();
                false
            }
        }
    }

    fn submit_soloist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.soloist.active().is_some() {
            return;
        }
        let account = self.spotify_account();
        match (&self.soloist.readiness, account) {
            (Some(Readiness::Installed { .. }), Some(account)) => {
                let key = self.soloist.key.read(cx).text().trim().to_owned();
                if key.is_empty() {
                    self.soloist.issue = Some("Введите ключ Spotify Soloist.".into());
                    self.soloist
                        .key
                        .update(cx, |field, cx| field.focus(window, cx));
                    cx.notify();
                    return;
                }
                self.soloist
                    .key
                    .update(cx, |field, cx| field.set_text("", cx));
                window.focus(&self.soloist.close_focus, cx);
                self.run_soloist(Work::Pair { account, key }, cx);
            }
            (Some(Readiness::Installed { .. }), None) => {
                self.close_soloist(window, cx);
                self.show_music_page(Page::Settings, window, cx);
                self.open_settings_connection(ProviderId::Spotify, window, cx);
            }
            (Some(Readiness::Ready { .. }), _) => self.finish_soloist(window, cx),
            (_, account) => self.run_soloist(Work::Install { account }, cx),
        }
    }

    fn finish_soloist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let retry = self
            .soloist
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.opened_for.as_ref())
            .is_some_and(|state| state == &self.player_snapshot && state.soloist_setup_required);
        self.close_soloist(window, cx);
        if retry {
            self.player.update(cx, |bar, cx| {
                bar.command(cx, crate::playback::Playback::toggle_pause)
            });
        }
    }
}
