//! Identity for marking the current recording across catalogue views.

use std::{borrow::Cow, path::Path};

use crate::{
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::ProviderId,
};

use super::{ObjectKind, ObjectRef, reference::valid_id};

pub(crate) enum Recording<'a> {
    Catalogue {
        provider: ProviderId,
        profile: Option<&'a str>,
        id: Cow<'a, str>,
        original: Option<&'a str>,
    },
    File(&'a Path),
    Url(&'a str),
}

impl<'a> Recording<'a> {
    pub(crate) fn from_track(key: &'a PlaylistKey, track: &'a PlaylistTrack) -> Option<Self> {
        let (provider, id, original) = match &track.media {
            Some(Media::File(path)) => return Some(Self::File(path)),
            Some(Media::Url(url)) => {
                let Some(reference) =
                    ObjectRef::from_url(url).filter(|r| r.kind == ObjectKind::Track)
                else {
                    return Some(Self::Url(url));
                };
                (reference.provider, Cow::Owned(reference.id), None)
            }
            Some(Media::Provider { provider, id }) => {
                // Spotify catalogue rows can carry both the requested ID and
                // its playable, relinked ID. Both identify the same recording.
                let original = (*provider == ProviderId::Spotify
                    && key.provider() == Some(*provider)
                    && track.id != *id
                    && valid_id(*provider, ObjectKind::Track, &track.id))
                .then_some(track.id.as_str());
                (*provider, Cow::Borrowed(id.as_str()), original)
            }
            None => (key.provider()?, Cow::Borrowed(track.id.as_str()), None),
        };
        let profile = key
            .connected_source()
            .filter(|&(source, _)| source == provider)
            .and_then(|(_, profile)| profile);
        valid_id(provider, ObjectKind::Track, &id).then_some(Self::Catalogue {
            provider,
            profile,
            id,
            original,
        })
    }

    pub(crate) fn from_reference(
        reference: &'a ObjectRef,
        profile: Option<&'a str>,
    ) -> Option<Self> {
        (reference.kind == ObjectKind::Track && reference.is_valid()).then_some(Self::Catalogue {
            provider: reference.provider,
            profile,
            id: Cow::Borrowed(&reference.id),
            original: None,
        })
    }

    pub(crate) fn matches(&self, other: &Recording<'_>) -> bool {
        match (self, other) {
            (Self::File(a), Recording::File(b)) => a == b,
            (Self::Url(a), Recording::Url(b)) => a == b && !a.is_empty(),
            (
                Self::Catalogue {
                    provider,
                    profile,
                    id,
                    original,
                },
                Recording::Catalogue {
                    provider: other_provider,
                    profile: other_profile,
                    id: other_id,
                    original: other_original,
                },
            ) if provider == other_provider => {
                if *provider == ProviderId::Jellyfin
                    && (profile.is_none() || profile != other_profile)
                {
                    return false;
                }
                std::iter::once(id.as_ref()).chain(*original).any(|id| {
                    std::iter::once(other_id.as_ref())
                        .chain(*other_original)
                        .any(|other| canonical_id(*provider, id) == canonical_id(*provider, other))
                })
            }
            _ => false,
        }
    }
}

fn canonical_id(provider: ProviderId, id: &str) -> &str {
    if provider == ProviderId::YandexMusic
        && let Some((track, _album)) = id.split_once(':')
        && track.bytes().all(|byte| byte.is_ascii_digit())
    {
        return track;
    }
    id
}
