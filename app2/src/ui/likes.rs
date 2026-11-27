mod catalog;
mod controls;
mod preferences;
mod view;

#[cfg(all(test, feature = "capture", target_os = "windows"))]
mod native;

pub(super) use controls::like_button;

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{Context, FocusHandle, Task, Window};

use crate::{
    music::likes::{self, Destination, LikeIdentity, Match, State, Subject},
    providers::ProviderId,
    storage::Store,
};

use super::MainView;
type Provider = Option<ProviderId>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum LikeAction {
    Save,
    Remove,
    Check,
    Pending { marked: bool },
}

impl LikeAction {
    fn for_destinations(
        selected: impl IntoIterator<Item = Provider>,
        mut marked: impl FnMut(Provider) -> Option<bool>,
    ) -> Self {
        let mut all_known = true;
        for provider in selected {
            match marked(provider) {
                Some(false) => return Self::Save,
                None => all_known = false,
                Some(true) => {}
            }
        }
        if all_known { Self::Remove } else { Self::Check }
    }
}

fn effective_defaults(
    defaults: &[Provider],
    available: impl Fn(Provider) -> bool,
) -> Vec<Provider> {
    let mut selected: Vec<_> = defaults
        .iter()
        .copied()
        .filter(|id| available(*id))
        .collect();
    if selected.is_empty() {
        selected.push(None);
    }
    selected
}

#[cfg(test)]
#[test]
fn quick_like_removes_only_when_every_default_is_marked() {
    let remote = Some(ProviderId::Jellyfin);
    let defaults = [None, remote];
    assert_eq!(
        LikeAction::for_destinations(defaults, |id| Some(id.is_none())),
        LikeAction::Save,
    );
    assert_eq!(
        LikeAction::for_destinations(defaults, |id| Some(id.is_some())),
        LikeAction::Save,
    );
    assert_eq!(
        LikeAction::for_destinations(defaults, |_| Some(true)),
        LikeAction::Remove,
    );
    assert_eq!(
        LikeAction::for_destinations(defaults, |id| id.is_none().then_some(true)),
        LikeAction::Check,
    );
}

#[derive(Clone, Copy)]
pub(super) enum Gesture {
    Activate,
    Open,
    BeginPress,
    EndPress,
}

struct Dialog {
    subject: Subject,
    visible: bool,
    return_focus: Option<FocusHandle>,
    choices: Option<Vec<Destination>>,
    named_errors: BTreeMap<Provider, String>,
    error: Option<String>,
    candidates: Option<(Destination, Vec<Match>)>,
    search: Option<(ProviderId, Work)>,
    status_checks: BTreeMap<ProviderId, Work>,
}

struct Work {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

struct PendingWrite {
    work: Work,
    profile: Option<String>,
    liked: bool,
}

impl PendingWrite {
    fn mark_for(&self, profile: Option<&str>) -> Option<bool> {
        (self.profile.as_deref() == profile && !self.work.cancelled.load(Ordering::Relaxed))
            .then_some(self.liked)
    }
}

struct Mutation {
    recording: Option<Arc<Match>>,
    liked: bool,
    explicit_match: bool,
}

impl Drop for Work {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct LikeUi {
    defaults: Vec<Provider>,
    settings_error: Option<String>,
    settings_task: Option<Task<()>>,
    cached_marks: HashMap<LikeIdentity, BTreeMap<Provider, bool>>,
    write_errors: HashMap<LikeIdentity, BTreeMap<Provider, String>>,
    accounts: BTreeMap<ProviderId, String>,
    writes: HashMap<(LikeIdentity, Provider), PendingWrite>,
    dialog: Option<Dialog>,
    loading: HashMap<LikeIdentity, Task<()>>,
    pressed: Option<Task<()>>,
    held: Option<LikeIdentity>,
    focus: Option<FocusHandle>,
}

impl LikeUi {
    fn profile(&self, provider: Provider) -> Option<&str> {
        provider.and_then(|provider| self.accounts.get(&provider).map(String::as_str))
    }

    fn pending_mark(
        &self,
        identity: &LikeIdentity,
        provider: Provider,
        profile: Option<&str>,
    ) -> Option<bool> {
        if self.writes.is_empty() {
            return None;
        }
        self.writes
            .get(&(identity.clone(), provider))?
            .mark_for(profile)
    }

    fn has_pending(&self, subject: &LikeIdentity) -> bool {
        self.writes.iter().any(|((identity, provider), write)| {
            identity == subject && write.mark_for(self.profile(*provider)).is_some()
        })
    }

    pub(super) fn cancel_account(&self, provider: ProviderId) {
        for ((_, destination), write) in &self.writes {
            if *destination == Some(provider) {
                write.work.cancelled.store(true, Ordering::Relaxed);
            }
        }
    }

    pub(super) fn remove_account(&mut self, provider: ProviderId) {
        self.writes
            .retain(|(_, destination), _| *destination != Some(provider));
        self.cached_marks.clear();
        self.loading.clear();
        self.dialog = None;
        self.write_errors.clear();
    }
}

impl MainView {
    pub(in crate::ui) fn sync_like_accounts(&mut self, cx: &mut Context<Self>) {
        let accounts = self
            .state
            .home_view()
            .map_or_else(BTreeMap::new, |(home, _)| {
                home.accounts
                    .iter()
                    .map(|account| (account.provider, account.profile.id.clone()))
                    .collect()
            });
        if accounts == self.likes.accounts {
            return;
        }
        for provider in ProviderId::ALL {
            if self.likes.accounts.get(&provider) != accounts.get(&provider) {
                self.likes.remove_account(provider);
            }
        }
        self.likes.accounts = accounts;
        self.update_player_favorite(cx);
    }

    pub(in crate::ui) fn open_like_destinations(
        &mut self,
        subject: Subject,
        quick: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disconnecting() || self.is_settings_login() {
            return;
        }
        let pending_read = self.likes.loading.contains_key(&subject.identity);
        let pending_write = self.likes.has_pending(&subject.identity);
        if quick && (pending_read || pending_write) {
            return;
        }
        if !quick
            && (pending_read || pending_write)
            && let Some(dialog) = self.likes.dialog.as_mut()
            && dialog.subject.identity == subject.identity
        {
            dialog.visible = true;
            self.focus_likes(window, cx);
            cx.notify();
            return;
        }
        self.likes.dialog = Some(Dialog {
            subject: subject.clone(),
            visible: !quick,
            return_focus: window.focused(cx),
            choices: None,
            named_errors: self
                .likes
                .write_errors
                .remove(&subject.identity)
                .unwrap_or_default(),
            error: None,
            candidates: None,
            search: None,
            status_checks: BTreeMap::new(),
        });
        if !quick {
            self.focus_likes(window, cx);
        }
        if pending_read {
            cx.notify();
            return;
        }
        let identity = subject.identity.clone();
        let task = cx.spawn_in(window, async move |view, cx| {
            let requested = subject.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default().map_err(|error| error.to_string())?;
                    likes::destinations(&store, &requested)
                })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.likes.loading.remove(&subject.identity);
                view.show_like_destinations(subject, result, quick, window, cx);
                cx.notify();
            });
        });
        self.likes.loading.insert(identity, task);
        cx.notify();
    }

    fn show_like_destinations(
        &mut self,
        subject: Subject,
        result: Result<Vec<Destination>, String>,
        quick: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .likes
            .dialog
            .as_mut()
            .filter(|dialog| dialog.subject.identity == subject.identity);
        let rows = match result {
            Ok(rows) => rows,
            Err(error) => {
                if let Some(dialog) = current {
                    dialog.error = Some(error);
                    dialog.visible = true;
                    self.focus_likes(window, cx);
                }
                return;
            }
        };
        if let Some(dialog) = current {
            dialog.choices = Some(rows.clone());
            self.cache_like_status(cx);
        }
        if quick {
            self.apply_default_likes(subject, &rows, window, cx);
        }
        if self
            .likes
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.visible)
        {
            self.check_unknown_like_marks(cx);
        }
    }

    fn apply_default_likes(
        &mut self,
        subject: Subject,
        rows: &[Destination],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let defaults = effective_defaults(&self.likes.defaults, |id| {
            rows.iter().any(|row| row.provider == id)
        });
        let selected = rows
            .iter()
            .filter(|row| defaults.contains(&row.provider))
            .collect::<Vec<_>>();
        let action = LikeAction::for_destinations(defaults, |id| {
            match &selected.iter().find(|row| row.provider == id)?.state {
                State::Local { liked } => Some(*liked),
                State::Ready { liked, .. } => *liked,
                State::Unmatched | State::Error(_) => None,
            }
        });
        let new_value = action != LikeAction::Remove;
        let needs_selection = selected
            .iter()
            .any(|row| matches!(row.state, State::Unmatched | State::Error(_)));
        if needs_selection
            && let Some(dialog) = self.likes.dialog.as_mut()
            && dialog.subject.identity == subject.identity
        {
            dialog.visible = true;
            self.focus_likes(window, cx);
        }
        if new_value {
            self.station_like_for_subject(&subject, cx);
        }
        for row in selected {
            let recording = match &row.state {
                State::Local { liked } if *liked != new_value => None,
                State::Ready { recording, liked } if *liked != Some(new_value) => {
                    Some(recording.clone())
                }
                _ => continue,
            };
            self.save_like(
                subject.clone(),
                row.clone(),
                Mutation {
                    recording,
                    liked: new_value,
                    explicit_match: false,
                },
                window,
                cx,
            );
        }
    }

    fn station_like_for_subject(&mut self, subject: &Subject, cx: &mut Context<Self>) {
        let key = self
            .player
            .read(cx)
            .current()
            .filter(|entry| {
                LikeIdentity::from_track(&entry.playlist, &entry.track).as_ref()
                    == Some(&subject.identity)
            })
            .map(|entry| (entry.playlist.clone(), entry.track.id.clone()));
        if let Some((key, id)) = key {
            self.station_like(&key, &id, cx);
        }
    }

    fn save_like(
        &mut self,
        subject: Subject,
        row: Destination,
        mutation: Mutation,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Mutation {
            recording,
            liked,
            explicit_match,
        } = mutation;
        let key = (subject.identity.clone(), row.provider);
        if self.likes.writes.contains_key(&key) || self.disconnecting() {
            return;
        }
        if let Some(dialog) = self.likes.dialog.as_mut() {
            dialog.named_errors.remove(&row.provider);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let tracked = key.clone();
        let profile = row.profile.clone();
        let finished_row = row.clone();
        let finished_recording = recording.clone();
        let task = cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default().map_err(|error| error.to_string())?;
                    match recording {
                        Some(recording) => likes::set_remote(
                            &store,
                            &subject,
                            &row,
                            &recording,
                            liked,
                            explicit_match,
                            &flag,
                        ),
                        None => likes::set_local(&store, &subject, liked, &flag),
                    }
                })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                let Some(write) = view.likes.writes.remove(&tracked) else {
                    return;
                };
                if write.mark_for(view.likes.profile(tracked.1)).is_none() {
                    view.update_player_favorite(cx);
                    cx.notify();
                    return;
                }
                let updated = view
                    .likes
                    .dialog
                    .as_mut()
                    .filter(|dialog| dialog.subject.identity == tracked.0);
                if let Err(error) = result {
                    if let Some(dialog) = updated {
                        dialog.named_errors.insert(finished_row.provider, error);
                        if !dialog.visible {
                            dialog.visible = true;
                            view.focus_likes(window, cx);
                        }
                    } else {
                        view.likes
                            .write_errors
                            .entry(tracked.0)
                            .or_default()
                            .insert(finished_row.provider, error);
                    }
                } else if let Ok(change) = result {
                    if let Some(dialog) = updated
                        && let Some(row) = dialog.choices.as_mut().and_then(|rows| {
                            rows.iter_mut().find(|row| {
                                row.provider == finished_row.provider
                                    && row.profile == finished_row.profile
                            })
                        })
                    {
                        row.state = match finished_recording {
                            Some(recording) => State::Ready {
                                recording,
                                liked: Some(liked),
                            },
                            None => State::Local { liked },
                        };
                    }
                    view.likes
                        .cached_marks
                        .entry(tracked.0.clone())
                        .or_default()
                        .insert(tracked.1, liked);
                    if tracked.1.is_none() {
                        view.playlist
                            .favorites
                            .changed_local_record(&tracked.0, liked);
                    }
                    view.apply_library_like(change);
                }
                view.update_player_favorite(cx);
                cx.notify();
            });
        });
        self.likes.writes.insert(
            key,
            PendingWrite {
                work: Work {
                    cancelled,
                    _task: task,
                },
                profile,
                liked,
            },
        );
        self.update_player_favorite(cx);
        cx.notify();
    }

    fn cache_like_status(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.likes.dialog.as_ref() else {
            return;
        };
        // This cache is for visible control state. Durable state lives in the profile store.
        const MARK_CACHE_LIMIT: usize = 128;
        if self.likes.cached_marks.len() >= MARK_CACHE_LIMIT {
            self.likes.cached_marks.clear();
        }
        let marks = dialog
            .choices
            .iter()
            .flatten()
            .filter_map(|row| {
                let liked = match row.state {
                    State::Local { liked }
                    | State::Ready {
                        liked: Some(liked), ..
                    } => liked,
                    _ => return None,
                };
                Some((row.provider, liked))
            })
            .collect();
        self.likes
            .cached_marks
            .insert(dialog.subject.identity.clone(), marks);
        self.update_player_favorite(cx);
    }
}
