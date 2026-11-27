mod download;
mod prefetch;
#[cfg(test)]
mod tests;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use async_channel::Sender;
use librespot_core::SpotifyUri;

use crate::{
    music::sound::{self, AlternateStream},
    playback::{
        QueueEntry, SoundSource,
        soloist::{Preparation, SoloistAudio},
    },
    playlist::Media,
    providers::{self, ProviderId, StreamOffer},
    storage::Store,
};

use super::{Failure, Message};

pub(super) use prefetch::{Prefetch, Ticket};

#[derive(Clone)]
pub(in crate::playback::worker) enum Input {
    Mpv(String),
    Soloist(SoloistAudio),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Candidate {
    File,
    Cached(PathBuf),
    Link,
    Download(ProviderId),
    Provider {
        provider: ProviderId,
        profile: String,
        id: String,
    },
}

#[derive(Clone)]
pub(in crate::playback::worker) struct Resolved {
    pub(super) input: Input,
    pub(super) source: SoundSource,
    pub(super) candidate: Candidate,
    pub(super) reason: String,
    pub(super) lease: Option<Arc<tempfile::TempPath>>,
}

pub(super) struct Resolve {
    pub generation: u64,
    pub entry: QueueEntry,
    pub cancelled: Arc<AtomicBool>,
    pub rejected: Vec<(Candidate, String)>,
    pub setup_failed: bool,
    pub prefetched: Option<Ticket>,
}

pub(super) struct Resolver(Sender<Resolve>);

impl Resolver {
    pub(super) fn spawn(sender: mpsc::Sender<Message>) -> Result<Self, String> {
        let (requests, receiver) = async_channel::bounded::<Resolve>(1);
        let downloader = download::Downloader::spawn(sender.clone())?;
        thread::Builder::new()
            .name("mediadeka-audio-source".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv_blocking() {
                    if request.cancelled.load(Ordering::Acquire) {
                        continue;
                    }
                    let input = match resolve(&request) {
                        Ok(Route::Ready(input)) => Ok(*input),
                        Ok(Route::Download(plan)) => {
                            if sender
                                .send(Message::Downloading {
                                    generation: request.generation,
                                })
                                .is_err()
                            {
                                break;
                            }
                            match downloader.request(*plan) {
                                Ok(()) => continue,
                                Err(error) => Err(error),
                            }
                        }
                        Err(error) => Err(error),
                    };
                    if !request.cancelled.load(Ordering::Acquire)
                        && sender
                            .send(Message::Resolved {
                                generation: request.generation,
                                input,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|_| "Не удалось запустить выбор источника звука.")?;
        Ok(Self(requests))
    }

    pub(super) fn request(&self, request: Resolve) -> Result<(), String> {
        self.0
            .force_send(request)
            .map(|_| ())
            .map_err(|_| "Выбор источника звука завершил работу.".into())
    }
}

pub(super) fn local_file(entry: &QueueEntry) -> Result<Option<Resolved>, String> {
    let Some(Media::File(path)) = &entry.track.media else {
        return Ok(None);
    };
    let input = file_input(path)?;
    Ok(Some(Resolved {
        input: Input::Mpv(input),
        source: SoundSource::LocalFile,
        candidate: Candidate::File,
        reason: "Файл пользователя".into(),
        lease: None,
    }))
}

fn file_input(path: &std::path::Path) -> Result<String, String> {
    path.to_str()
        .filter(|_| path.is_absolute() && path.is_file())
        .map(str::to_owned)
        .ok_or_else(|| "Аудиофайл по сохранённому пути недоступен.".into())
}

enum Route {
    Ready(Box<Resolved>),
    Download(Box<download::Plan>),
}

fn resolve(request: &Resolve) -> Result<Route, Failure> {
    let mut decision = Decision::new(
        &request.cancelled,
        &request.rejected,
        request.setup_failed,
        Preparation::Playback,
    );
    if let Ok(Some(file)) = local_file(&request.entry) {
        return Ok(Route::Ready(Box::new(file)));
    }
    decision.check_cancelled()?;
    let store = Store::open_default();
    if let Ok(store) = &store {
        match store.cached_audio(&request.entry) {
            Ok(Some(cached)) => {
                let candidate = Candidate::Cached(cached.path.clone());
                if !decision.rejected(&candidate)
                    && let Ok(input) = file_input(&cached.path)
                {
                    return Ok(Route::Ready(Box::new(decision.accept(Resolved {
                        input: Input::Mpv(input),
                        source: SoundSource::Cached,
                        candidate,
                        reason: format!("Проверенный файл профиля · {}", cached.quality.label()),
                        lease: None,
                    }))));
                }
            }
            Ok(None) => {}
            Err(error) => decision.note(format!("Кэш: {error}")),
        }
    }

    let prefetched = request.prefetched.as_ref().and_then(Ticket::ready);
    if let Some(link) = decision.link(&request.entry) {
        return Ok(Route::Ready(Box::new(decision.accept(link))));
    }
    if let Ok(store) = &store
        && let Some(stream) = decision.streams(store, &request.entry, prefetched)?
    {
        return Ok(Route::Ready(Box::new(decision.accept(stream))));
    }
    match store {
        Ok(store) if !decision.downloads.is_empty() => {
            return Ok(Route::Download(Box::new(download::Plan {
                store,
                entry: request.entry.clone(),
                generation: request.generation,
                cancelled: Arc::clone(&request.cancelled),
                candidates: decision.downloads,
                notes: decision.notes,
                setup_failure: decision.setup_failure,
            })));
        }
        Ok(_) => {}
        Err(error) => decision.note(format!("Профиль: {error}")),
    }
    decision.no_source()
}

fn speculative(entry: &QueueEntry, cancelled: &AtomicBool) -> Option<Resolved> {
    let store = Store::open_default().ok()?;
    if store.cached_audio(entry).ok().flatten().is_some() {
        return None;
    }
    let mut decision = Decision::new(cancelled, &[], false, Preparation::Prefetch);
    decision
        .streams(&store, entry, None)
        .ok()
        .flatten()
        .map(|stream| decision.accept(stream))
}

struct Decision<'a> {
    cancelled: &'a AtomicBool,
    rejected: &'a [(Candidate, String)],
    tried: Vec<Candidate>,
    downloads: Vec<Candidate>,
    notes: Vec<String>,
    setup_failure: bool,
    preparation: Preparation,
}

impl<'a> Decision<'a> {
    fn new(
        cancelled: &'a AtomicBool,
        rejected: &'a [(Candidate, String)],
        setup_failure: bool,
        preparation: Preparation,
    ) -> Self {
        Self {
            cancelled,
            rejected,
            tried: Vec::new(),
            downloads: Vec::new(),
            notes: rejected.iter().map(|(_, reason)| reason.clone()).collect(),
            setup_failure,
            preparation,
        }
    }

    fn check_cancelled(&self) -> Result<(), Failure> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err("Выбор источника отменён.".into());
        }
        Ok(())
    }

    fn rejected(&self, candidate: &Candidate) -> bool {
        self.rejected.iter().any(|(key, _)| key == candidate) || self.tried.contains(candidate)
    }

    fn note(&mut self, note: String) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    fn accept(self, mut selection: Resolved) -> Resolved {
        if !self.notes.is_empty() {
            selection.reason = format!("{} · {}", selection.reason, self.notes.join(" · "));
        }
        selection
    }

    fn no_source(mut self) -> Result<Route, Failure> {
        self.check_cancelled()?;
        self.note("В кэше нет готовой копии. Источники загрузки не подтвердили запись.".into());
        let reason = self.notes.join(" · ");
        if self.setup_failure {
            Err(Failure::SoloistSetup(reason))
        } else {
            Err(Failure::Track(reason))
        }
    }

    fn link(&mut self, entry: &QueueEntry) -> Option<Resolved> {
        let Some(Media::Url(url)) = &entry.track.media else {
            return None;
        };
        if self.rejected(&Candidate::Link) {
            return None;
        }
        match valid_url(url) {
            Ok(()) => Some(Resolved {
                input: Input::Mpv(url.clone()),
                source: SoundSource::Link,
                candidate: Candidate::Link,
                reason: "Аудиопоток по ссылке".into(),
                lease: None,
            }),
            Err(error) => {
                self.note(error);
                None
            }
        }
    }

    fn streams(
        &mut self,
        store: &Store,
        entry: &QueueEntry,
        prefetched: Option<Resolved>,
    ) -> Result<Option<Resolved>, Failure> {
        self.check_cancelled()?;
        let Some(recording) = sound::recording(&entry.playlist, &entry.track) else {
            return Ok(None);
        };
        let origin = recording.provider;
        let mut prefetched = prefetched.filter(|stream| !self.rejected(&stream.candidate));
        let explicit_profile = entry
            .playlist
            .connected_source()
            .filter(|(provider, _)| *provider == origin)
            .and_then(|(_, profile)| profile);
        let account = match explicit_profile {
            Some(profile) => store.account(origin, profile),
            None => store.preferred_account(origin),
        };
        match account {
            Ok(Some(account)) => {
                let candidate = Candidate::Provider {
                    provider: origin,
                    profile: account.profile.id,
                    id: recording.id.clone(),
                };
                let reason = "Полный поток исходной площадки";
                if let Some(stream) =
                    self.direct(store, entry, candidate, reason, &mut prefetched)?
                {
                    return Ok(Some(stream));
                }
            }
            Ok(None) => self.note(format!("{}: профиль не подключён", origin.display_name())),
            Err(error) => self.note(format!("{}: {error}", origin.display_name())),
        }
        self.check_cancelled()?;
        if let Some(stream) = prefetched.take()
            && let Candidate::Provider {
                provider, profile, ..
            } = &stream.candidate
            && *provider != origin
            && store
                .preferred_account(*provider)
                .is_ok_and(|account| account.is_some_and(|active| active.profile.id == *profile))
        {
            return Ok(Some(stream));
        }

        match sound::confirmed_streams(store, &entry.playlist, &entry.track) {
            Ok(links) => {
                if let Some(stream) = self.alternates(store, entry, links)? {
                    return Ok(Some(stream));
                }
            }
            Err(error) => self.note(error),
        }
        self.check_cancelled()?;
        let isrc = match sound::recording_isrc(store, &entry.playlist, &entry.track, self.cancelled)
        {
            Ok(Some(isrc)) => isrc,
            Ok(None) => return Ok(None),
            Err(error) => {
                self.note(error);
                return Ok(None);
            }
        };
        self.check_cancelled()?;
        match sound::alternate_streams(
            store,
            &entry.playlist,
            &entry.track,
            Some(&isrc),
            self.cancelled,
        ) {
            Ok(streams) => self.alternates(store, entry, streams),
            Err(error) => {
                self.note(error);
                Ok(None)
            }
        }
    }

    fn alternates(
        &mut self,
        store: &Store,
        entry: &QueueEntry,
        streams: Vec<AlternateStream>,
    ) -> Result<Option<Resolved>, Failure> {
        for stream in streams {
            let candidate = Candidate::Provider {
                provider: stream.provider,
                profile: stream.profile,
                id: stream.id,
            };
            if let Some(selection) =
                self.direct(store, entry, candidate, stream.evidence, &mut None)?
            {
                return Ok(Some(selection));
            }
        }
        Ok(None)
    }

    fn direct(
        &mut self,
        store: &Store,
        entry: &QueueEntry,
        candidate: Candidate,
        reason: &str,
        prefetched: &mut Option<Resolved>,
    ) -> Result<Option<Resolved>, Failure> {
        self.check_cancelled()?;
        let Candidate::Provider {
            provider,
            profile,
            id,
        } = &candidate
        else {
            unreachable!("direct playback requires a provider");
        };
        if providers::downloads::PROVIDERS.contains(provider)
            && !self.downloads.contains(&candidate)
            && !self.rejected(&Candidate::Download(*provider))
        {
            self.downloads.push(candidate.clone());
        }
        if self.rejected(&candidate) {
            return Ok(None);
        }
        let result = (|| {
            let account = store
                .account(*provider, profile)
                .map_err(|error| error.to_string())?
                .ok_or("Профиль отключён.")?;
            let credentials = store
                .credentials(&account)
                .map_err(|error| error.to_string())?
                .ok_or("Вход в профиль нужно подтвердить заново.")?;
            if let Some(stream) = prefetched.take_if(|stream| stream.candidate == candidate) {
                return Ok((stream.input, None));
            }
            if *provider == ProviderId::Spotify {
                let uri = SpotifyUri::from_uri(&format!("spotify:track:{id}"))
                    .map_err(|_| "Некорректный идентификатор трека Spotify.")?;
                return SoloistAudio::prepare(
                    uri,
                    entry
                        .track
                        .duration
                        .map(|seconds| u64::from(seconds) * 1000),
                    profile.to_owned(),
                    self.cancelled,
                    self.preparation,
                )
                .map(|audio| (Input::Soloist(audio), None));
            }
            match providers::direct_stream(*provider, Some(&credentials), id, self.cancelled)
                .map_err(|error| error.to_string())?
            {
                StreamOffer::Full(stream) => {
                    valid_url(&stream.url)?;
                    let mut format = stream.codec;
                    if let Some(kbps) = stream.bitrate_kbps {
                        let bitrate = format!("{kbps} кбит/с");
                        format = Some(
                            format.map_or(bitrate.clone(), |codec| format!("{codec}, {bitrate}")),
                        );
                    }
                    Ok((Input::Mpv(stream.url), format))
                }
                StreamOffer::Preview => Err("Площадка предложила только отрывок.".into()),
            }
        })();
        self.check_cancelled()?;
        let source = match &result {
            Ok((Input::Soloist(_), _)) => SoundSource::Soloist,
            _ => SoundSource::Stream(*provider),
        };
        match result {
            Ok((input, declared_format)) => Ok(Some(Resolved {
                input,
                source,
                candidate,
                reason: declared_format.map_or_else(
                    || reason.to_owned(),
                    |format| format!("{reason} · Площадка сообщает {format}"),
                ),
                lease: None,
            })),
            Err(error) => {
                self.setup_failure |= matches!(error, Failure::SoloistSetup(_));
                self.note(format!(
                    "{}: {}",
                    provider.display_name(),
                    error.into_message()
                ));
                self.tried.push(candidate);
                Ok(None)
            }
        }
    }
}

fn valid_url(value: &str) -> Result<(), String> {
    if !url::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
    {
        return Err("Источник вернул неподдерживаемый адрес аудио.".into());
    }
    Ok(())
}
