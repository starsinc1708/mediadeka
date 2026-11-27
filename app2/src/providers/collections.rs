use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::playlist::PlaylistTrack;

use super::{
    Credentials, HomeItem, Profile, ProviderError, jellyfin, soundcloud, spotify,
    transport::check_cancelled, yandex, youtube,
};

pub(super) const MAX_PAGES: usize = 1000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub(crate) enum CollectionKind {
    Liked,
    Albums,
    Artists,
    Playlists,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum Collection {
    Liked(Vec<PlaylistTrack>),
    Albums(Vec<SavedItem>),
    Artists(Vec<SavedItem>),
    Playlists(Vec<HomeItem>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedItem {
    pub item: HomeItem,
    pub added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
}

pub(crate) fn read(
    credentials: &Credentials,
    profile: &Profile,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<Collection, ProviderError> {
    check_cancelled(cancelled)?;
    let result = match credentials {
        Credentials::Spotify { .. } => {
            spotify::read_collection(credentials, profile, kind, cancelled)
        }
        Credentials::YouTubeMusic { .. } => {
            youtube::read_collection(credentials, profile, kind, cancelled)
        }
        Credentials::YandexMusic { access_token, .. } => {
            yandex::read_collection(access_token, &profile.id, kind, cancelled)
        }
        Credentials::SoundCloud { token, client_id } => {
            soundcloud::read_collection(token, client_id, &profile.id, kind, cancelled)
        }
        Credentials::Jellyfin {
            server,
            device,
            access_token,
        } => jellyfin::read_collection(server, device, access_token, &profile.id, kind, cancelled),
    };
    check_cancelled(cancelled)?;
    result
}
