use std::collections::{HashMap, HashSet};

use gpui::{Context, Task};

use crate::{
    music::likes::{LikeIdentity, remote_liked_tracks},
    playlist::PlaylistKey,
    providers::ProviderId,
    storage::{FavoriteScope, Store, StoreError},
};

use super::MainView;

#[cfg(all(test, feature = "capture"))]
mod tests;

#[derive(Clone, Eq, Hash, PartialEq)]
enum Scope {
    Local,
    Guest(ProviderId),
    Account(ProviderId, String),
}

impl From<&PlaylistKey> for Scope {
    fn from(key: &PlaylistKey) -> Self {
        match key.connected_source() {
            None => Self::Local,
            Some((provider, Some(profile))) => Self::Account(provider, profile.to_owned()),
            Some((provider, None)) => Self::Guest(provider),
        }
    }
}

impl Scope {
    fn of_identity(identity: &LikeIdentity) -> Self {
        match (identity.provider, &identity.profile) {
            (Some(provider), Some(profile)) => Self::Account(provider, profile.clone()),
            (Some(provider), None) => Self::Guest(provider),
            (None, _) => Self::Local,
        }
    }

    fn as_store_scope(&self) -> FavoriteScope<'_> {
        match self {
            Self::Local => FavoriteScope::Local,
            Self::Guest(provider) => FavoriteScope::Guest(*provider),
            Self::Account(provider, profile) => FavoriteScope::Account(*provider, profile),
        }
    }

    fn belongs_to_account(&self, provider: ProviderId) -> bool {
        matches!(self, Self::Account(owner, _) if *owner == provider)
    }
}

enum Read {
    Loading { _task: Task<()> },
    Loaded(ScopeMarks),
    Failed,
}

#[derive(Default)]
struct ScopeMarks {
    local: HashSet<String>,
    remote: HashSet<String>,
    has_remote_snapshot: bool,
}

#[derive(Default)]
pub(in crate::ui) struct Marks {
    reads: HashMap<Scope, Read>,
}

impl Marks {
    #[cfg(all(test, feature = "capture"))]
    fn mark(&self, key: &PlaylistKey, id: &str) -> Option<bool> {
        match self.reads.get(&Scope::from(key))? {
            Read::Loaded(marks) => Some(marks.local.contains(id)),
            _ => None,
        }
    }

    pub(in crate::ui) fn destination_mark(
        &self,
        identity: &LikeIdentity,
        destination: Option<ProviderId>,
    ) -> Option<bool> {
        let Read::Loaded(marks) = self.reads.get(&Scope::of_identity(identity))? else {
            return None;
        };
        match destination {
            None => Some(marks.local.contains(&identity.id)),
            Some(provider) if identity.provider == Some(provider) && identity.profile.is_some() => {
                let marked = marks.remote.contains(&identity.id);
                (marked || marks.has_remote_snapshot).then_some(marked)
            }
            Some(_) => None,
        }
    }

    pub(super) fn read_failed(&self, key: &PlaylistKey) -> bool {
        matches!(self.reads.get(&Scope::from(key)), Some(Read::Failed))
    }

    fn finish_read(&mut self, scope: &Scope, result: Result<ScopeMarks, StoreError>) -> bool {
        let Some(read @ Read::Loading { .. }) = self.reads.get_mut(scope) else {
            return false;
        };
        *read = match result {
            Ok(tracks) => Read::Loaded(tracks),
            Err(error) => {
                eprintln!("Не удалось прочитать избранное Mediadeka: {error}");
                Read::Failed
            }
        };
        true
    }

    fn remove_account(&mut self, provider: ProviderId) {
        self.reads
            .retain(|scope, _| !scope.belongs_to_account(provider));
    }

    pub(in crate::ui) fn changed_local_record(&mut self, identity: &LikeIdentity, favorite: bool) {
        let Some(Read::Loaded(marks)) = self.reads.get_mut(&Scope::of_identity(identity)) else {
            return;
        };
        if favorite {
            marks.local.insert(identity.id.clone());
        } else {
            marks.local.remove(&identity.id);
        }
    }
}

impl MainView {
    pub(in crate::ui) fn ensure_playlist_favorites(
        &mut self,
        key: &PlaylistKey,
        cx: &mut Context<Self>,
    ) {
        let scope = Scope::from(key);
        if !self.playlist.favorites.reads.contains_key(&scope) {
            self.read_favorites(scope, cx);
        }
    }

    pub(super) fn retry_playlist_favorites(&mut self, key: &PlaylistKey, cx: &mut Context<Self>) {
        let scope = Scope::from(key);
        if self.playlist.favorites.read_failed(key) {
            self.read_favorites(scope, cx);
        }
    }

    fn read_favorites(&mut self, scope: Scope, cx: &mut Context<Self>) {
        let requested = scope.clone();
        let task = cx.spawn(async move |view, cx| {
            let work = requested.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default()?;
                    let mut marks = ScopeMarks {
                        local: store.local_like_ids(work.as_store_scope())?,
                        ..ScopeMarks::default()
                    };
                    if let Scope::Account(provider, profile) = &work
                        && let Some(account) = store.account(*provider, profile)?
                    {
                        let (tracks, has_snapshot) =
                            remote_liked_tracks(&store, &account, &crate::music::region())?;
                        marks
                            .remote
                            .extend(tracks.into_iter().map(|track| track.id));
                        marks.has_remote_snapshot = has_snapshot;
                    }
                    Ok(marks)
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.playlist.favorites.finish_read(&requested, result) {
                    view.update_player_favorite(cx);
                    cx.notify();
                }
            });
        });
        self.playlist
            .favorites
            .reads
            .insert(scope, Read::Loading { _task: task });
        cx.notify();
    }

    pub(in crate::ui) fn cancel_account_favorite_changes(&self, provider: ProviderId) {
        self.likes.cancel_account(provider);
    }

    pub(in crate::ui) fn remove_account_favorites(
        &mut self,
        provider: ProviderId,
        cx: &mut Context<Self>,
    ) {
        self.playlist.favorites.remove_account(provider);
        self.likes.remove_account(provider);
        self.update_player_favorite(cx);
    }

    pub(in crate::ui) fn update_player_favorite(&mut self, cx: &mut Context<Self>) {
        let liked = self
            .player
            .read(cx)
            .current()
            .and_then(|entry| self.track_like_action(&entry.playlist, &entry.track, &[]));
        self.player.update(cx, |bar, cx| bar.set_liked(liked, cx));
    }
}
