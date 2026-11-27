use std::sync::atomic::AtomicBool;

use super::{
    Credentials, ErrorKind, ProviderError, ProviderId, catalog::missing_access, jellyfin,
    soundcloud, transport::check_cancelled, yandex,
};

pub(crate) struct DirectStream {
    pub url: String,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
}

pub(crate) enum StreamOffer {
    Full(DirectStream),
    Preview,
}

pub(crate) fn direct_stream(
    provider: ProviderId,
    credentials: Option<&Credentials>,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<StreamOffer, ProviderError> {
    check_cancelled(cancelled)?;
    let result = match (provider, credentials) {
        (ProviderId::YandexMusic, Some(Credentials::YandexMusic { access_token, .. })) => {
            yandex::direct_stream(access_token, id, cancelled)
        }
        (ProviderId::SoundCloud, Some(Credentials::SoundCloud { token, client_id })) => {
            soundcloud::direct_stream(token, client_id, id, cancelled)
        }
        (
            ProviderId::Jellyfin,
            Some(Credentials::Jellyfin {
                server,
                device,
                access_token,
            }),
        ) => jellyfin::track_stream(server, device, access_token, id).map(|url| {
            StreamOffer::Full(DirectStream {
                url,
                codec: None,
                bitrate_kbps: None,
            })
        }),
        (ProviderId::Spotify | ProviderId::YouTubeMusic, _) => Err(ProviderError::new(
            ErrorKind::Unavailable,
            format!(
                "Аудиопоток {} недоступен в Mediadeka.",
                provider.display_name()
            ),
        )),
        _ => Err(missing_access(provider)),
    };
    check_cancelled(cancelled)?;
    result
}
