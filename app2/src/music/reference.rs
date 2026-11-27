use serde::{Deserialize, Serialize};
use url::Url;

use crate::providers::{HomeItem, ProviderId, playlist_id};

use super::links::public_link;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ObjectKind {
    Artist,
    Album,
    Track,
    Playlist,
    Collection,
    Station,
}

impl ObjectKind {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Track => "track",
            Self::Playlist => "playlist",
            Self::Collection => "genre",
            Self::Station => "station",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Artist => "Артист",
            Self::Album => "Альбом",
            Self::Track => "Трек",
            Self::Playlist => "Плейлист",
            Self::Collection => "Подборка",
            Self::Station => "Станция",
        }
    }

    fn from_path(path: &str) -> Option<Self> {
        match path {
            "artist" => Some(Self::Artist),
            "album" => Some(Self::Album),
            "track" => Some(Self::Track),
            "playlist" => Some(Self::Playlist),
            "page" | "genre" => Some(Self::Collection),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub provider: ProviderId,
    pub kind: ObjectKind,
    pub id: String,
}

impl ObjectRef {
    pub(crate) fn native(provider: ProviderId, kind: ObjectKind, id: String) -> Option<Self> {
        valid_id(provider, kind, &id).then_some(Self { provider, kind, id })
    }

    pub(crate) fn from_item(provider: ProviderId, item: &HomeItem) -> Option<Self> {
        use ObjectKind as K;
        use ProviderId as P;
        let card_id = item.id.as_str();
        let station = match provider {
            P::YandexMusic => card_id.strip_prefix("wave:"),
            P::SoundCloud => card_id.strip_prefix("station:").or_else(|| {
                crate::providers::soundcloud_artist_station_id(card_id).then_some(card_id)
            }),
            _ => None,
        };
        if let Some(id) = station
            && valid_id(provider, K::Station, id)
        {
            return Some(Self {
                provider,
                kind: K::Station,
                id: id.to_owned(),
            });
        }
        if provider == P::YandexMusic
            && let Some(link) = item.url.as_deref().and_then(Self::from_url)
            && link.provider == provider
            && link.kind == K::Collection
        {
            return Some(link);
        }
        let (kind, id) = match provider {
            P::Spotify => card_id
                .strip_prefix("spotify:")?
                .split_once(':')
                .and_then(|(kind, id)| Some((K::from_path(kind)?, id.to_owned()))),
            P::YouTubeMusic => {
                if let Some(id) = card_id.strip_prefix("browse:") {
                    if id.starts_with("MPRE") {
                        Some((K::Album, id.to_owned()))
                    } else if id.starts_with("UC") {
                        Some((K::Artist, id.to_owned()))
                    } else {
                        playlist_id(provider, item).map(|id| (K::Playlist, id))
                    }
                } else {
                    card_id
                        .strip_prefix("video:")
                        .map(|id| (K::Track, id.to_owned()))
                }
            }
            _ => playlist_id(provider, item)
                .map(|id| (K::Playlist, id))
                .or_else(|| {
                    card_id
                        .split_once(':')
                        .and_then(|(kind, id)| Some((K::from_path(kind)?, id.to_owned())))
                        .or_else(|| {
                            ((provider == P::SoundCloud && digits(card_id))
                                || (provider == P::Jellyfin
                                    && card_id.len() == 32
                                    && card_id.bytes().all(|byte| byte.is_ascii_hexdigit())))
                            .then(|| (K::Track, card_id.to_owned()))
                        })
                }),
        }?;
        valid_id(provider, kind, &id).then_some(Self { provider, kind, id })
    }

    pub(crate) fn from_url(value: &str) -> Option<Self> {
        let link = public_link(value)?;
        let provider = ProviderId::ALL
            .into_iter()
            .find(|provider| provider.as_str() == link.platform)?;
        Some(Self {
            provider,
            kind: link.kind,
            id: link.id,
        })
    }

    pub(crate) fn url(&self) -> Option<String> {
        use ObjectKind as K;
        use ProviderId as P;
        let id = self.id.as_str();
        if !valid_id(self.provider, self.kind, id) {
            return None;
        }
        let url = match (self.provider, self.kind) {
            (P::Spotify, kind) => format!("https://open.spotify.com/{}/{id}", kind.path()),
            (P::YouTubeMusic, K::Track) => format!("https://music.youtube.com/watch?v={id}"),
            (P::YouTubeMusic, K::Playlist) => {
                format!("https://music.youtube.com/playlist?list={id}")
            }
            (P::YouTubeMusic, K::Album) if id.starts_with("OLAK5uy_") => {
                format!("https://music.youtube.com/playlist?list={id}")
            }
            (P::YouTubeMusic, _) => format!("https://music.youtube.com/browse/{id}"),
            (P::YandexMusic, K::Track) => {
                let (track, album) = id.split_once(':')?;
                format!("https://music.yandex.ru/album/{album}/track/{track}")
            }
            (P::YandexMusic, K::Playlist) => return crate::providers::yandex::playlist_url(id),
            (P::YandexMusic, K::Collection) => return crate::providers::yandex::tag_url(id),
            (P::YandexMusic, K::Station) => return None,
            (P::YandexMusic, kind) => format!("https://music.yandex.ru/{}/{id}", kind.path()),
            (P::SoundCloud, K::Station) => return crate::providers::soundcloud_station_url(id),
            (P::SoundCloud, _) if id.contains('/') || (self.kind == K::Artist && !digits(id)) => {
                format!("https://soundcloud.com/{id}")
            }
            _ => return None,
        };
        Some(url)
    }

    pub(crate) fn is_valid(&self) -> bool {
        valid_id(self.provider, self.kind, &self.id)
    }
}

fn digits(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|character| character.is_ascii_digit())
}

fn simple_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
}

pub(super) fn valid_id(provider: ProviderId, kind: ObjectKind, id: &str) -> bool {
    use ObjectKind as K;
    use ProviderId as P;
    match (provider, kind) {
        (P::YandexMusic, K::Station) => crate::providers::yandex::station::valid_seed(id),
        (P::SoundCloud, K::Station) => crate::providers::soundcloud_station_id(id),
        (_, K::Station) => false,
        (P::Spotify, _) => id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        (P::YouTubeMusic, K::Track) => id.len() == 11 && simple_id(id),
        (P::YouTubeMusic, _) => simple_id(id),
        (P::YandexMusic, K::Playlist) => crate::providers::yandex::valid_playlist_id(id),
        (P::YandexMusic, K::Collection) => crate::providers::yandex::tag_slug(id).is_some(),
        (P::YandexMusic, K::Track) => {
            digits(id)
                || id
                    .split_once(':')
                    .is_some_and(|(a, b)| simple_id(a) && digits(b))
        }
        (P::YandexMusic, _) => digits(id),
        (P::SoundCloud, K::Playlist) => id.split(':').all(simple_id),
        (P::SoundCloud, _) => id.split('/').all(simple_id),
        (P::Jellyfin, _) => simple_id(id),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SearchInput {
    Url(String),
    Text { kind: ObjectKind, query: String },
}

impl SearchInput {
    pub(crate) fn new(kind: ObjectKind, query: &str) -> Option<Self> {
        let query = query.trim();
        if query.is_empty() || query.len() > 2048 {
            return None;
        }
        let is_link =
            Url::parse(query).is_ok_and(|url| url.has_host() || public_link(query).is_some());
        Some(if is_link {
            Self::Url(query.to_owned())
        } else {
            Self::Text {
                kind,
                query: query.to_owned(),
            }
        })
    }
}

pub(crate) fn search_url(provider: ProviderId, query: &str) -> Option<Url> {
    let (endpoint, parameter) = match provider {
        ProviderId::Spotify => ("https://open.spotify.com/search/", None),
        ProviderId::YouTubeMusic => ("https://music.youtube.com/search", Some("q")),
        ProviderId::YandexMusic => ("https://music.yandex.ru/search", Some("text")),
        ProviderId::SoundCloud => ("https://soundcloud.com/search", Some("q")),
        ProviderId::Jellyfin => return None,
    };
    let mut url = Url::parse(endpoint).ok()?;
    if let Some(parameter) = parameter {
        url.query_pairs_mut().append_pair(parameter, query);
    } else {
        url.path_segments_mut().ok()?.pop_if_empty().push(query);
    }
    Some(url)
}
