use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use crate::{
    music::{
        ObjectKind,
        likes::{LikeIdentity, remote_liked_tracks},
        matching::MatchLinks,
        normalized, region,
    },
    playback::QueueEntry,
    playlist::PlaylistKey,
    storage::{Store, StoreError},
};

const MIX_LENGTH: usize = 20;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Mode {
    #[default]
    Familiar,
    Discoveries,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Reaction {
    MoreLikeThis,
    Hide,
}

pub(crate) fn set_reaction(
    store: &Store,
    entry: &QueueEntry,
    reaction: Reaction,
) -> Result<(), StoreError> {
    if entry.recommendations.is_empty() {
        return Err(StoreError::InvalidData);
    }
    let identity =
        LikeIdentity::from_track(&entry.playlist, &entry.track).ok_or(StoreError::InvalidData)?;
    let artist = (reaction == Reaction::MoreLikeThis).then(|| normalized(&entry.track.artist));
    store.write_recommendation_reaction(
        &identity,
        reaction,
        artist.as_deref().filter(|credit| !credit.is_empty()),
    )
}

enum Signal {
    Liked,
    Played,
    Reaction(Reaction, Option<String>),
}

#[derive(Default)]
struct Recording<'a> {
    liked: bool,
    played: bool,
    excluded: bool,
    reaction: Option<(Reaction, Option<&'a str>)>,
    preferred_artist: bool,
}

/// Local data read once per batch, reusable if the queue changes during assembly.
pub(crate) struct Ranking {
    mode: Mode,
    links: MatchLinks,
    signals: Vec<(LikeIdentity, Signal)>,
}

impl Ranking {
    pub(crate) fn load(store: &Store, mode: Mode) -> Result<Self, StoreError> {
        let region = region();
        let links = MatchLinks::load(store, &region, ObjectKind::Track)?;
        let mut signals = Vec::new();
        if mode == Mode::Familiar {
            signals.extend(
                liked_identities(store, &region)?
                    .into_iter()
                    .map(|identity| (identity, Signal::Liked)),
            );
        }
        signals.extend(store.listening_history()?.into_iter().filter_map(|row| {
            let identity = LikeIdentity::from_track(&row.key, &row.track)?;
            Some((identity, Signal::Played))
        }));
        // SQLite returns reactions in write order. The latest action wins across confirmed aliases.
        signals.extend(
            store
                .recommendation_reactions()?
                .into_iter()
                .map(|(identity, reaction, artist)| (identity, Signal::Reaction(reaction, artist))),
        );
        Ok(Self {
            mode,
            links,
            signals,
        })
    }

    #[cfg(all(test, windows))]
    pub(super) fn unranked(store: &Store) -> Result<Self, StoreError> {
        Ok(Self::without_signals(MatchLinks::load(
            store,
            &region(),
            ObjectKind::Track,
        )?))
    }

    #[cfg(test)]
    pub(super) fn without_signals(links: MatchLinks) -> Self {
        Self {
            mode: Mode::default(),
            links,
            signals: Vec::new(),
        }
    }

    pub(crate) fn entries(
        &self,
        candidates: &[QueueEntry],
        excluded: &[QueueEntry],
    ) -> Vec<QueueEntry> {
        let groups = self.links.groups(
            candidates
                .iter()
                .chain(excluded)
                .map(|entry| LikeIdentity::from_track(&entry.playlist, &entry.track))
                .chain(
                    self.signals
                        .iter()
                        .map(|(identity, _)| Some(identity.clone())),
                )
                .map(|identity| {
                    (
                        identity.as_ref().and_then(LikeIdentity::reference),
                        identity,
                    )
                }),
        );
        let (candidate_groups, rest) = groups.split_at(candidates.len());
        let (excluded_groups, signal_groups) = rest.split_at(excluded.len());
        let mut state: Vec<_> = (0..groups.len()).map(|_| Recording::default()).collect();
        for &group in excluded_groups {
            state[group].excluded = true;
        }
        for (&group, (_, signal)) in signal_groups.iter().zip(&self.signals) {
            let recording = &mut state[group];
            match signal {
                Signal::Liked => recording.liked = true,
                Signal::Played => recording.played = true,
                Signal::Reaction(reaction, artist) => {
                    recording.reaction = Some((*reaction, artist.as_deref()));
                }
            }
        }
        let preferred_artists: HashSet<_> = state
            .iter()
            .filter_map(|recording| match recording.reaction {
                Some((Reaction::MoreLikeThis, artist)) => artist,
                _ => None,
            })
            .collect();
        if !preferred_artists.is_empty() {
            for (&group, candidate) in candidate_groups.iter().zip(candidates) {
                state[group].preferred_artist |=
                    preferred_artists.contains(normalized(&candidate.track.artist).as_str());
            }
        }

        let mut seen = HashSet::new();
        let mut selected: Vec<_> = candidate_groups
            .iter()
            .copied()
            .enumerate()
            .filter(|&(_, group)| {
                let recording = &state[group];
                !recording.excluded
                    && !matches!(recording.reaction, Some((Reaction::Hide, _)))
                    && seen.insert(group)
            })
            .collect();
        selected.sort_by_key(|&(_, group)| {
            let recording = &state[group];
            let preferred = match self.mode {
                Mode::Familiar => recording.liked || recording.played,
                Mode::Discoveries => !recording.played,
            };
            let promoted = matches!(recording.reaction, Some((Reaction::MoreLikeThis, _)));
            Reverse((preferred, promoted, !promoted && recording.preferred_artist))
        });
        selected.truncate(MIX_LENGTH);

        let mut positions = HashMap::new();
        let mut entries = Vec::with_capacity(selected.len());
        for (index, group) in selected {
            positions.insert(group, entries.len());
            entries.push(candidates[index].clone());
        }
        for (&group, candidate) in candidate_groups.iter().zip(candidates) {
            let Some(&position) = positions.get(&group) else {
                continue;
            };
            let origins = &mut entries[position].recommendations;
            for origin in &candidate.recommendations {
                if !origins.contains(origin) {
                    origins.push(origin.clone());
                }
            }
        }
        entries
    }
}

fn liked_identities(store: &Store, region: &str) -> Result<HashSet<LikeIdentity>, StoreError> {
    let mut identities = store.local_like_identities()?;
    for summary in store.accounts()? {
        let provider = summary.provider;
        let profile = summary.profile.id;
        let Some(account) = store.account(provider, &profile)? else {
            continue;
        };
        let (tracks, _) = remote_liked_tracks(store, &account, region)?;
        let key = PlaylistKey::Source {
            provider,
            profile: Some(profile),
            id: String::new(),
        };
        identities.extend(
            tracks
                .iter()
                .filter_map(|track| LikeIdentity::from_track(&key, track)),
        );
    }
    Ok(identities)
}
