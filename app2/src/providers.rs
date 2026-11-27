pub(crate) mod catalog;
pub(crate) mod collections;
pub(crate) mod downloads;
pub(crate) mod genius;
mod jellyfin;
pub(crate) mod likes;
#[cfg(test)]
pub(crate) mod live_tests;
pub(crate) mod lrclib;
pub(crate) mod lyrics;
mod playlists;
mod soundcloud;
pub(crate) use soundcloud::track_radio as soundcloud_track_radio;
#[cfg(all(test, windows, feature = "capture"))]
pub(crate) use soundcloud::use_loopback_for_test as use_soundcloud_loopback_for_test;
pub(crate) use spotify::artist_radio as spotify_artist_radio;
pub(crate) use spotify::track_radio as spotify_track_radio;
mod spotify;
mod streams;
mod transport;
pub(crate) mod yandex;

pub(crate) use soundcloud::{
    artist_station_from_slug as soundcloud_artist_station_from_slug,
    artist_station_id as soundcloud_artist_station_id,
    artist_station_url as soundcloud_station_url, system_playlist_id as soundcloud_station_id,
};
mod youtube;

use std::{fmt, path::Path, sync::atomic::AtomicBool};

use serde::{Deserialize, Serialize};

pub(crate) use playlists::{playlist_id, read_playlist, read_playlist_with_progress};
pub use soundcloud::SoundCloudShelf;
pub(crate) use soundcloud::SoundCloudUpdate;
#[cfg(test)]
pub(crate) use soundcloud::load_soundcloud_home_for_test;
pub(crate) use spotify::refresh_library_access as refresh_spotify_library_access;
pub(crate) use streams::{StreamOffer, direct_stream};
pub(crate) use transport::{artwork_provider, is_public_artwork_url, soundcloud_card_artwork};
pub use youtube::{YouTubeChoices, youtube_profiles};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Spotify,
    YouTubeMusic,
    YandexMusic,
    #[serde(rename = "soundcloud")]
    SoundCloud,
    Jellyfin,
}

impl ProviderId {
    pub const ALL: [Self; 5] = [
        Self::Spotify,
        Self::YouTubeMusic,
        Self::YandexMusic,
        Self::SoundCloud,
        Self::Jellyfin,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::YouTubeMusic => "youtube-music",
            Self::YandexMusic => "yandex-music",
            Self::SoundCloud => "soundcloud",
            Self::Jellyfin => "jellyfin",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::YouTubeMusic => "YouTube Music",
            Self::YandexMusic => "Яндекс Музыка",
            Self::SoundCloud => "SoundCloud",
            Self::Jellyfin => "Jellyfin",
        }
    }
}

pub(crate) fn provider_index(provider: ProviderId) -> usize {
    ProviderId::ALL
        .into_iter()
        .position(|entry| entry == provider)
        .expect("built-in provider")
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub home: bool,
    pub profile_selection: bool,
    pub device_code: bool,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpotifySessionCredentials {
    pub username: String,
    pub auth_data: Vec<u8>,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum Credentials {
    Spotify {
        client_id: String,
        access_token: String,
        refresh_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_expires_at: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        librespot: Option<SpotifySessionCredentials>,
    },
    YouTubeMusic {
        cookies: String,
        index: usize,
        page_id: Option<String>,
    },
    YandexMusic {
        access_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_expires_at: Option<u64>,
    },
    #[serde(rename = "soundcloud")]
    SoundCloud { token: String, client_id: String },
    Jellyfin {
        server: String,
        device: String,
        access_token: String,
    },
}

impl Credentials {
    pub(crate) fn renewal_deadline(&self) -> Option<u64> {
        match self {
            Self::Spotify {
                refresh_token,
                access_expires_at,
                librespot: Some(_),
                ..
            }
            | Self::YandexMusic {
                refresh_token: Some(refresh_token),
                access_expires_at,
                ..
            } if transport::valid_token(refresh_token, 4096) => *access_expires_at,
            _ => None,
        }
    }
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Spotify { .. } => ProviderId::Spotify,
            Self::YouTubeMusic { .. } => ProviderId::YouTubeMusic,
            Self::YandexMusic { .. } => ProviderId::YandexMusic,
            Self::SoundCloud { .. } => ProviderId::SoundCloud,
            Self::Jellyfin { .. } => ProviderId::Jellyfin,
        }
    }
}

#[derive(Clone)]
pub struct Session {
    pub provider: ProviderId,
    pub profile: Profile,
    pub credentials: Credentials,
    pub capabilities: Capabilities,
}

impl Session {
    fn new(profile: Profile, credentials: Credentials) -> Self {
        let provider = credentials.provider();
        Self {
            provider,
            profile,
            credentials,
            capabilities: Capabilities {
                home: true,
                profile_selection: provider == ProviderId::YouTubeMusic,
                device_code: matches!(provider, ProviderId::YandexMusic | ProviderId::Jellyfin),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HomeItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork: Option<String>,
}

impl HomeItem {
    pub(crate) fn is_artist(&self) -> bool {
        self.id.starts_with("artist:")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Home {
    pub title: String,
    pub items: Vec<HomeItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<HomeSection>,
}

impl Home {
    pub(crate) fn personal_mix(&self, provider: ProviderId, index: usize) -> bool {
        use ProviderId as P;
        self.items.get(index).is_some_and(|item| match provider {
            P::Spotify | P::YouTubeMusic => self.sections.iter().any(|section| {
                (section.personal
                    || (provider == P::YouTubeMusic && youtube::is_mix_shelf(&section.title)))
                    && (section.start..section.start.saturating_add(section.len)).contains(&index)
            }),
            P::SoundCloud => soundcloud::personal_playlist_id(&item.id),
            P::YandexMusic => matches!(
                item.title.to_lowercase().as_str(),
                "плейлист дня" | "дежавю" | "премьера" | "тайник"
            ),
            P::Jellyfin => false,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct HomeSection {
    pub title: String,
    pub start: usize,
    pub len: usize,
    pub quick: bool,
    #[serde(default)]
    pub personal: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<HomeTab>,
    #[serde(default, skip_serializing_if = "HomeSectionStyle::is_default")]
    pub style: HomeSectionStyle,
    #[serde(default, skip_serializing_if = "HomeSectionPage::is_default")]
    pub page: HomeSectionPage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soundcloud_shelf: Option<SoundCloudShelf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HomeTab {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork: Option<String>,
    pub indices: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum HomeSectionStyle {
    #[default]
    Cards,
    Releases,
    Collections,
    Artists,
}

impl HomeSectionStyle {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum HomeSectionPage {
    #[default]
    Home,
    Activity,
    Library,
}

impl HomeSectionPage {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Network,
    Rejected,
    Expired,
    Revoked,
    Cancelled,
    InvalidInput,
    InvalidResponse,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ProviderError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn needs_sign_in(&self) -> bool {
        matches!(self.kind, ErrorKind::Expired | ErrorKind::Revoked)
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

pub enum LoginInput {
    Spotify,
    YouTubeMusic {
        cookies: String,
    },
    ChooseYouTube {
        choices: YouTubeChoices,
        id: String,
    },
    YandexDevice,
    SoundCloudBrowser {
        owner: isize,
    },
    JellyfinPassword {
        server: String,
        username: String,
        password: String,
    },
    JellyfinQuickConnect {
        server: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoginPrompt {
    Browser { url: String },
    Code { code: String, url: String },
    Waiting { message: String },
}

pub fn sign_in(
    provider: ProviderId,
    input: LoginInput,
    cancelled: &AtomicBool,
    prompt: impl Fn(LoginPrompt),
) -> Result<Session, ProviderError> {
    transport::check_cancelled(cancelled)?;
    let result = match (provider, input) {
        (ProviderId::Spotify, LoginInput::Spotify) => spotify::sign_in(cancelled, prompt),
        (ProviderId::YouTubeMusic, LoginInput::YouTubeMusic { cookies }) => {
            youtube::sign_in(cookies, cancelled)
        }
        (ProviderId::YouTubeMusic, LoginInput::ChooseYouTube { choices, id }) => {
            choices.choose(&id, cancelled)
        }
        (ProviderId::YandexMusic, LoginInput::YandexDevice) => yandex::sign_in(cancelled, prompt),
        (ProviderId::SoundCloud, LoginInput::SoundCloudBrowser { owner }) => {
            soundcloud::browser_sign_in(owner, cancelled, prompt)
        }
        (
            ProviderId::Jellyfin,
            LoginInput::JellyfinPassword {
                server,
                username,
                password,
            },
        ) => jellyfin::sign_in(&server, &username, &password),
        (ProviderId::Jellyfin, LoginInput::JellyfinQuickConnect { server }) => {
            jellyfin::quick_connect(&server, cancelled, prompt)
        }
        _ => Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Способ входа недоступен для этой площадки.",
        )),
    };
    transport::check_cancelled(cancelled)?;
    result
}

pub fn guest_home(cancelled: &AtomicBool) -> Result<Home, ProviderError> {
    youtube::home(None, cancelled)
}

pub fn restore(
    provider: ProviderId,
    previous_profile: &Profile,
    credentials: Credentials,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    if provider != credentials.provider() {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Данные подключения принадлежат другой площадке.",
        ));
    }
    match provider {
        ProviderId::Spotify => spotify::restore(credentials, previous_profile, cancelled),
        ProviderId::YouTubeMusic => youtube::restore(credentials, previous_profile, cancelled),
        ProviderId::YandexMusic => yandex::restore(credentials, cancelled),
        ProviderId::SoundCloud => soundcloud::restore(credentials, cancelled),
        ProviderId::Jellyfin => jellyfin::restore(credentials),
    }
}

pub(crate) fn restore_in_profile(
    provider: ProviderId,
    previous: &Profile,
    credentials: Credentials,
    directory: &Path,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    if provider == ProviderId::SoundCloud && provider == credentials.provider() {
        return soundcloud::restore_in_profile(credentials, previous, directory, cancelled);
    }
    restore(provider, previous, credentials, cancelled)
}

pub(crate) use soundcloud::clear_browser_session as clear_soundcloud_browser_session;

pub(crate) fn renew(
    mut session: Session,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    transport::check_cancelled(cancelled)?;
    match session.provider {
        ProviderId::Spotify => {
            spotify::refresh_access_token(&mut session.credentials)?;
            Ok(session)
        }
        ProviderId::YandexMusic => yandex::renew(session.credentials, cancelled),
        _ => Ok(session),
    }
}

pub(crate) use transport::agent as http_agent;
pub(crate) use transport::epoch_seconds;

pub fn home(session: &Session, cancelled: &AtomicBool) -> Result<Home, ProviderError> {
    Ok(match load_home(session, cancelled)? {
        HomeUpdate::Complete(home) => home,
        HomeUpdate::SoundCloud(update) => update.apply(None).0,
    })
}

pub(crate) enum HomeUpdate {
    Complete(Home),
    SoundCloud(Box<SoundCloudUpdate>),
}

pub(crate) fn load_home(
    session: &Session,
    cancelled: &AtomicBool,
) -> Result<HomeUpdate, ProviderError> {
    transport::check_cancelled(cancelled)?;
    match &session.credentials {
        Credentials::Spotify { .. } => spotify::home(session, cancelled),
        Credentials::YouTubeMusic { .. } => youtube::home(Some(&session.credentials), cancelled),
        Credentials::YandexMusic { access_token, .. } => {
            yandex::home(access_token, &session.profile.id, cancelled)
        }
        Credentials::SoundCloud { token, client_id } => {
            return soundcloud::home(token, client_id, &session.profile.id, cancelled)
                .map(Box::new)
                .map(HomeUpdate::SoundCloud);
        }
        Credentials::Jellyfin {
            server,
            device,
            access_token,
        } => jellyfin::home(server, device, access_token, &session.profile.id),
    }
    .map(HomeUpdate::Complete)
}
