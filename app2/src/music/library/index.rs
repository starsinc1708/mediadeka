use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, NaiveDate};

use crate::{
    music::{ObjectKind, ObjectRef, likes::LikeIdentity, normalized},
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::{HomeItem, ProviderId, collections::Collection},
};

use super::MatchLinks;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Scope {
    Local,
    Provider {
        provider: ProviderId,
        profile: String,
    },
}

impl Scope {
    pub(crate) fn provider(&self) -> Option<ProviderId> {
        match self {
            Self::Local => None,
            Self::Provider { provider, .. } => Some(*provider),
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.provider()
            .map_or("Mediadeka", ProviderId::display_name)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum LibraryGroup {
    Album { title: String, artist: String },
    Artist(String),
}

impl LibraryGroup {
    pub(crate) fn title(&self) -> &str {
        match self {
            Self::Album { title, .. } | Self::Artist(title) => title,
        }
    }

    pub(crate) fn contains(&self, track: &PlaylistTrack) -> bool {
        match self {
            Self::Album { title, artist } => {
                track.album.as_deref() == Some(title) && track.artist == *artist
            }
            Self::Artist(artist) => track.artist == *artist,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum EntryId {
    Source(String),
    LocalPlaylist(i64),
    LocalGroup(LibraryGroup),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct EntryKey(pub Scope, pub EntryId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EntryAction {
    Track(PlaylistKey, PlaylistTrack),
    Object {
        profile: String,
        object: ObjectRef,
        item: HomeItem,
    },
    LocalPlaylist {
        id: i64,
        track_count: usize,
    },
    LocalGroup(LibraryGroup),
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub key: EntryKey,
    pub title: String,
    pub subtitle: String,
    pub artwork: Option<String>,
    pub year: Option<u32>,
    pub added: Option<i64>,
    pub downloaded: bool,
    pub liked: bool,
    pub action: EntryAction,
    tokens: String,
    title_sort: String,
    subtitle_sort: String,
}

impl Entry {
    #[cfg(feature = "capture")]
    pub(crate) fn sample_file_for_capture(track: PlaylistTrack) -> Self {
        Self::track(Scope::Local, PlaylistKey::Local(0), track, false)
    }

    pub(super) fn track(scope: Scope, key: PlaylistKey, track: PlaylistTrack, liked: bool) -> Self {
        let downloaded = matches!(&track.media, Some(Media::File(path)) if path.is_file());
        let title_sort = normalized(&track.title);
        let subtitle_sort = normalized(&track.artist);
        let album = track.album.as_deref().map(normalized).unwrap_or_default();
        let tokens = format!("{title_sort} {subtitle_sort} {album}");
        Self {
            key: EntryKey(scope, EntryId::Source(track.id.clone())),
            title: track.title.clone(),
            subtitle: track.artist.clone(),
            artwork: track.artwork.clone(),
            year: None,
            added: track.added.as_deref().and_then(date),
            downloaded,
            liked,
            action: EntryAction::Track(key, track),
            title_sort,
            subtitle_sort,
            tokens,
        }
    }

    pub(super) fn item(
        key: EntryKey,
        item: &HomeItem,
        added: Option<&str>,
        downloaded: bool,
        action: EntryAction,
    ) -> Self {
        let title_sort = normalized(&item.title);
        let subtitle_sort = normalized(&item.subtitle);
        Self {
            key,
            title: item.title.clone(),
            subtitle: item.subtitle.clone(),
            artwork: item.artwork.clone(),
            year: None,
            added: added.and_then(date),
            downloaded,
            liked: false,
            action,
            tokens: format!("{title_sort} {subtitle_sort}"),
            title_sort,
            subtitle_sort,
        }
    }

    fn identity(&self) -> Option<LikeIdentity> {
        match &self.action {
            EntryAction::Track(key, track) => LikeIdentity::from_track(key, track),
            _ => None,
        }
    }

    pub(crate) fn object(&self) -> Option<ObjectRef> {
        match &self.action {
            EntryAction::Track(_, _) => {
                let identity = self.identity()?;
                Some(ObjectRef {
                    provider: identity.provider?,
                    kind: ObjectKind::Track,
                    id: identity.id,
                })
            }
            EntryAction::Object { object, .. } => Some(object.clone()),
            _ => None,
        }
    }

    pub(super) fn matches_word(&self, word: &str) -> bool {
        self.tokens.contains(word)
    }
}

fn date(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .or_else(|| DateTime::parse_from_str(value, "%Y/%m/%d %H:%M:%S %z").ok())
        .map(|date| date.timestamp())
        .or_else(|| {
            Some(
                NaiveDate::parse_from_str(value, "%Y-%m-%d")
                    .ok()?
                    .and_hms_opt(0, 0, 0)?
                    .and_utc()
                    .timestamp(),
            )
        })
}

pub(crate) fn source_entries(
    provider: ProviderId,
    profile: &str,
    collection: Collection,
) -> Vec<Entry> {
    let scope = Scope::Provider {
        provider,
        profile: profile.to_owned(),
    };
    let object = |item: HomeItem, added: Option<&str>, year| {
        let target = ObjectRef::from_item(provider, &item)?;
        let mut entry = Entry::item(
            EntryKey(scope.clone(), EntryId::Source(item.id.clone())),
            &item,
            added,
            false,
            EntryAction::Object {
                profile: profile.to_owned(),
                object: target,
                item: item.clone(),
            },
        );
        entry.year = year;
        Some(entry)
    };
    match collection {
        Collection::Liked(tracks) => tracks
            .into_iter()
            .map(|track| remote_liked_track(scope.clone(), provider, profile, track))
            .collect(),
        Collection::Albums(items) | Collection::Artists(items) => items
            .into_iter()
            .filter_map(|item| object(item.item, item.added.as_deref(), item.year))
            .collect(),
        Collection::Playlists(items) => items
            .into_iter()
            .filter_map(|item| object(item, None, None))
            .collect(),
    }
}

pub(super) fn remote_liked_track(
    scope: Scope,
    provider: ProviderId,
    profile: &str,
    track: PlaylistTrack,
) -> Entry {
    Entry::track(
        scope,
        PlaylistKey::Source {
            provider,
            profile: Some(profile.to_owned()),
            id: format!("track:{}", track.id),
        },
        track,
        true,
    )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LibrarySort {
    #[default]
    Recent,
    Title,
    Artist,
}

impl LibrarySort {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Recent => "По дате добавления",
            Self::Title => "По названию",
            Self::Artist => "По исполнителю",
        }
    }
}

struct Member {
    batch: Arc<[Entry]>,
    index: usize,
    cached: bool,
}

impl Member {
    fn entry(&self) -> &Entry {
        &self.batch[self.index]
    }

    fn downloaded(&self) -> bool {
        self.cached || self.entry().downloaded
    }
}

pub(crate) struct LibraryRow {
    members: Vec<Member>,
    pub downloaded: bool,
    pub added: Option<i64>,
}

impl LibraryRow {
    pub(crate) fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.members.iter().map(Member::entry)
    }

    pub(crate) fn entry(&self, scope: Option<&Scope>) -> &Entry {
        let in_scope = |entry: &&Entry| scope.is_none_or(|scope| *scope == entry.key.0);
        self.members
            .iter()
            .filter(|member| in_scope(&member.entry()))
            .find(|member| member.downloaded())
            .map(Member::entry)
            .or_else(|| self.entries().find(in_scope))
            .unwrap_or_else(|| self.members[0].entry())
    }

    pub(crate) fn scopes(&self) -> impl Iterator<Item = &Scope> {
        self.entries().enumerate().filter_map(|(index, entry)| {
            (!self
                .entries()
                .take(index)
                .any(|other| other.key.0 == entry.key.0))
            .then_some(&entry.key.0)
        })
    }
}

#[derive(Default)]
pub(crate) struct LibraryIndex {
    pub rows: Vec<LibraryRow>,
}

#[derive(Default)]
pub(crate) struct LibraryResults {
    pub visible: Vec<usize>,
    pub counts: BTreeMap<Option<ProviderId>, usize>,
    pub visible_sources: usize,
    pub unfiltered: usize,
    pub downloaded: usize,
    pub first_added: Option<i64>,
}

impl LibraryIndex {
    #[cfg(test)]
    pub(crate) fn build(
        batches: impl IntoIterator<Item = Arc<[Entry]>>,
        matches: &MatchLinks,
    ) -> Self {
        Self::build_with_changes(batches, matches, &BTreeMap::new())
    }

    pub(crate) fn build_with_changes(
        batches: impl IntoIterator<Item = Arc<[Entry]>>,
        matches: &MatchLinks,
        changes: &BTreeMap<EntryKey, Option<Arc<[Entry]>>>,
    ) -> Self {
        let mut members = Vec::new();
        let mut keys = HashSet::new();
        for batch in batches {
            for (index, entry) in batch.iter().enumerate() {
                if !keys.insert(entry.key.clone()) {
                    continue;
                }
                let (batch, index) = match changes.get(&entry.key) {
                    Some(None) => continue,
                    // Keep the server's metadata/date for a track already present.
                    Some(Some(replacement)) if entry.key.0 == Scope::Local => {
                        (replacement.clone(), 0)
                    }
                    _ => (batch.clone(), index),
                };
                members.push(Member {
                    batch,
                    index,
                    cached: false,
                });
            }
        }
        for (key, entries) in changes {
            if !keys.contains(key)
                && let Some(entries) = entries
            {
                members.push(Member {
                    batch: entries.clone(),
                    index: 0,
                    cached: false,
                });
            }
        }
        let matching = matches.groups(members.iter().map(|member| {
            let entry = member.entry();
            (entry.object(), entry.identity())
        }));
        let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (group, member) in matching.into_iter().zip(members) {
            groups.entry(group).or_default().push(member);
        }
        let rows = groups
            .into_values()
            .map(|members| LibraryRow {
                downloaded: members.iter().any(Member::downloaded),
                added: members
                    .iter()
                    .filter_map(|member| member.entry().added)
                    .min(),
                members,
            })
            .collect();
        Self { rows }
    }

    pub(crate) fn apply_downloads(&mut self, cache: &HashSet<LikeIdentity>) {
        for row in &mut self.rows {
            for member in &mut row.members {
                member.cached = !cache.is_empty()
                    && member
                        .entry()
                        .identity()
                        .is_some_and(|identity| cache.contains(&identity));
            }
            row.downloaded = row.members.iter().any(Member::downloaded);
        }
    }

    pub(crate) fn project(
        &self,
        scope: Option<&Scope>,
        group: Option<&LibraryGroup>,
        query: &str,
        downloaded: bool,
        sort: LibrarySort,
    ) -> LibraryResults {
        let query = normalized(query);
        let words: Vec<_> = query.split_whitespace().collect();
        let mut result = LibraryResults::default();
        let mut providers = std::collections::HashSet::new();
        for (index, row) in self.rows.iter().enumerate() {
            if downloaded && !row.downloaded {
                continue;
            }
            if !words
                .iter()
                .all(|word| row.entries().any(|entry| entry.matches_word(word)))
            {
                continue;
            }
            if let Some(group) = group
                && !row.entries().any(|entry| {
                    entry.key.0 == Scope::Local
                        && matches!(&entry.action, EntryAction::Track(_, track) if group.contains(track))
                })
            {
                continue;
            }
            result.unfiltered += 1;
            for source in row.scopes() {
                *result.counts.entry(source.provider()).or_default() += 1;
            }
            if scope.is_some_and(|scope| !row.scopes().any(|source| source == scope)) {
                continue;
            }
            result.visible.push(index);
            result.downloaded += usize::from(row.downloaded);
            providers.extend(row.scopes().filter_map(Scope::provider));
            result.first_added = result.first_added.into_iter().chain(row.added).min();
        }
        result.visible_sources = providers.len();
        result.visible.sort_by(|&a, &b| {
            let left = &self.rows[a];
            let right = &self.rows[b];
            let a = left.entry(scope);
            let b = right.entry(scope);
            let order = match sort {
                LibrarySort::Recent => right.added.cmp(&left.added),
                LibrarySort::Title => std::cmp::Ordering::Equal,
                LibrarySort::Artist => a.subtitle_sort.cmp(&b.subtitle_sort),
            };
            order.then_with(|| a.title_sort.cmp(&b.title_sort))
        });
        result
    }
}
