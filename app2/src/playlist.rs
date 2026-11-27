pub(crate) mod library;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::providers::ProviderId;

pub use crate::music::{ObjectKind, ObjectRef};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub owner: Option<PlaylistOwner>,
    pub artwork: Option<String>,
    pub link: Option<String>,
    pub tracks: Vec<PlaylistTrack>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlaylistOwner {
    Named(String),
    Profile { name: String, reference: ObjectRef },
}

impl PlaylistOwner {
    pub(crate) fn new(name: String, reference: Option<ObjectRef>) -> Self {
        match reference {
            Some(reference) => Self::Profile { name, reference },
            None => Self::Named(name),
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Named(name) | Self::Profile { name, .. } => name,
        }
    }

    pub(crate) fn reference(&self) -> Option<&ObjectRef> {
        match self {
            Self::Named(_) => None,
            Self::Profile { reference, .. } => Some(reference),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    /// Individual credits in catalogue order, when the provider exposes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<String>,
    /// Individually attributed names from the provider, in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artist_credits: Vec<ArtistCredit>,
    pub album: Option<String>,
    /// The release identified by its source catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_ref: Option<ObjectRef>,
    pub artwork: Option<String>,
    pub added: Option<String>,
    pub duration: Option<u32>,
    pub media: Option<Media>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtistCredit {
    pub name: String,
    pub reference: Option<ObjectRef>,
}

impl ArtistCredit {
    pub(crate) fn from_catalog(name: String, provider: ProviderId, id: Option<String>) -> Self {
        let reference = id.and_then(|id| ObjectRef::native(provider, ObjectKind::Artist, id));
        Self { name, reference }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Media {
    File(PathBuf),
    Url(String),
    Provider { provider: ProviderId, id: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum PlaylistKey {
    Local(i64),
    Source {
        provider: ProviderId,
        profile: Option<String>,
        id: String,
    },
    Station {
        provider: ProviderId,
        profile: Option<String>,
        id: String,
    },
}

impl PlaylistKey {
    pub(crate) fn provider(&self) -> Option<ProviderId> {
        match self {
            Self::Local(_) => None,
            Self::Source { provider, .. } | Self::Station { provider, .. } => Some(*provider),
        }
    }

    pub(crate) fn connected_source(&self) -> Option<(ProviderId, Option<&str>)> {
        match self {
            Self::Local(_) => None,
            Self::Source {
                provider, profile, ..
            }
            | Self::Station {
                provider, profile, ..
            } => Some((*provider, profile.as_deref())),
        }
    }
}

#[cfg(test)]
#[test]
fn cached_playlist_author_keeps_legacy_name_and_round_trips_typed_profile() {
    let mut playlist: Playlist = serde_json::from_value(serde_json::json!({
        "name": "Saved playlist", "owner": "Saved author", "tracks": [],
    }))
    .unwrap();
    let legacy = playlist.owner.take().unwrap();
    assert_eq!(legacy.name(), "Saved author");
    assert!(legacy.reference().is_none());
    assert_eq!(serde_json::to_value(legacy).unwrap(), "Saved author");
    playlist.owner = Some(PlaylistOwner::Profile {
        name: "Confirmed author".into(),
        reference: ObjectRef::native(ProviderId::SoundCloud, ObjectKind::Artist, "42".into())
            .unwrap(),
    });
    let encoded = serde_json::to_vec(&playlist).unwrap();
    assert_eq!(
        serde_json::from_slice::<Playlist>(&encoded).unwrap(),
        playlist
    );
}
