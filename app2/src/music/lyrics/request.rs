use std::{
    cmp::Reverse,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use crate::{
    music::{ObjectKind, ObjectRef, links, region, service::cached_links, sound},
    playback::{QueueEntry, SoundSource},
    playlist::Media,
    providers::{ProviderId, lrclib, lyrics as native},
    storage::{Account, Store},
};

use super::{
    Lookup, LyricsVersions, Recording, Source, Timing, http_cache, local,
    matching::youtube_song_title,
};

mod pending;
use pending::{ABSENCE_RETENTION, Key, LOCAL_RETENTION, Loaded, NETWORK};

#[cfg(test)]
mod acceptance;

const NATIVE_SOURCES: [Source; 3] = [Source::YandexMusic, Source::Spotify, Source::YouTubeMusic];

enum Request {
    Local(PathBuf),
    Lrclib {
        youtube_title: Option<String>,
    },
    PublicYouTube {
        id: String,
        legacy_account: Option<Account>,
    },
    Native {
        source: Source,
        reference: ObjectRef,
        account: Account,
    },
}

impl Request {
    fn source(&self) -> Source {
        match self {
            Self::Local(_) => Source::Local,
            Self::Lrclib { .. } => Source::Lrclib,
            Self::PublicYouTube { .. } => Source::YouTubeMusic,
            Self::Native { source, .. } => *source,
        }
    }

    fn account(&self) -> Option<&Account> {
        match self {
            Self::Native { account, .. } => Some(account),
            Self::Local(_) | Self::Lrclib { .. } | Self::PublicYouTube { .. } => None,
        }
    }

    fn cache_keys(&self, recording: &Recording) -> [Option<String>; 2] {
        match self {
            Self::Local(_) => [None, None],
            Self::Lrclib { youtube_title } => {
                let original = serde_json::to_string(&(Source::Lrclib, recording)).ok();
                match youtube_title {
                    Some(title) => [
                        serde_json::to_string(&(Source::Lrclib, recording, title)).ok(),
                        original,
                    ],
                    None => [original, None],
                }
            }
            Self::PublicYouTube { id, .. } => [native_key(Source::YouTubeMusic, id), None],
            Self::Native {
                source, reference, ..
            } => native_keys(*source, &reference.id, recording),
        }
    }

    fn fetch(
        &self,
        store: &Store,
        recording: &Recording,
        cancelled: &AtomicBool,
    ) -> Result<Lookup, String> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Lookup::Indeterminate);
        }
        match self {
            Self::Local(path) => Ok(local::read(path).map_or(Lookup::Indeterminate, |versions| {
                Lookup::Found(versions, None)
            })),
            Self::Lrclib { youtube_title } => {
                lrclib::fetch(recording, youtube_title.as_deref(), cancelled)
            }
            Self::PublicYouTube { id, .. } => native::fetch_public_youtube(id, cancelled),
            Self::Native {
                reference, account, ..
            } => {
                let credentials = store.credentials(account).map_err(display)?;
                let Some(credentials) = credentials.filter(native::supports) else {
                    return Ok(Lookup::Indeterminate);
                };
                if !store.lyrics_account_is_current(account).map_err(display)? {
                    return Ok(Lookup::Indeterminate);
                }
                native::fetch(reference.provider, &credentials, &reference.id, cancelled).map(
                    |result| match result {
                        Some(versions) => {
                            let max_age = (!versions.needs_format_revalidation())
                                .then_some(http_cache::MAX_AGE);
                            Lookup::Found(versions, max_age)
                        }
                        None if reference.provider == ProviderId::Spotify => {
                            Lookup::Missing(Some(http_cache::MAX_AGE))
                        }
                        None => Lookup::Indeterminate,
                    },
                )
            }
        }
    }

    fn pending_key(&self, store: &Store, key: Option<&str>) -> Option<Key> {
        match self {
            Self::Local(path) => Some(Key::Local(path.clone(), local::revision(path))),
            _ => key.map(|key| Key::Remote {
                directory: store.directory().to_owned(),
                account: self.account().cloned(),
                key: key.to_owned(),
            }),
        }
    }

    fn legacy_lyrics(
        &self,
        store: &Store,
        recording: &Recording,
        key: Option<&str>,
    ) -> Result<Option<Arc<LyricsVersions>>, String> {
        let Self::PublicYouTube {
            id,
            legacy_account: Some(account),
        } = self
        else {
            return Ok(None);
        };
        let now = http_cache::now();
        if let Some(key) = key
            && let Some(snapshot) = store.lyrics_snapshot(key, None, now).map_err(display)?
        {
            return Ok(Some(Arc::new(snapshot.versions)));
        }
        for key in native_keys(Source::YouTubeMusic, id, recording)
            .into_iter()
            .flatten()
        {
            if let Some(snapshot) = store
                .lyrics_snapshot(&key, Some(account), now)
                .map_err(display)?
            {
                return Ok(Some(Arc::new(snapshot.versions)));
            }
        }
        Ok(None)
    }
}

fn native_key(source: Source, id: &str) -> Option<String> {
    serde_json::to_string(&(source, id)).ok()
}

fn native_keys(source: Source, id: &str, recording: &Recording) -> [Option<String>; 2] {
    [
        native_key(source, id),
        serde_json::to_string(&(source, id, recording)).ok(),
    ]
}

pub(crate) fn load(
    store: &Store,
    entry: &QueueEntry,
    sound: Option<&SoundSource>,
    manual_source: Option<Source>,
    cancelled: &AtomicBool,
    emit: impl FnMut(LyricsVersions),
) -> Result<(), String> {
    load_requested(
        store,
        entry,
        sound,
        manual_source,
        cancelled,
        Purpose::Current,
        emit,
    )
}

pub(crate) fn reload(
    store: &Store,
    entry: &QueueEntry,
    sound: Option<&SoundSource>,
    manual_source: Option<Source>,
    cancelled: &AtomicBool,
    emit: impl FnMut(LyricsVersions),
) -> Result<(), String> {
    load_requested(
        store,
        entry,
        sound,
        manual_source,
        cancelled,
        Purpose::Retry,
        emit,
    )
}

pub(crate) fn prefetch(
    store: &Store,
    entry: &QueueEntry,
    cancelled: &AtomicBool,
    emit: impl FnMut(LyricsVersions),
) -> Result<(), String> {
    load_requested(store, entry, None, None, cancelled, Purpose::Upcoming, emit)
}

#[cfg(feature = "capture")]
pub(crate) fn seed_lrclib_versions_for_capture(
    store: &Store,
    entry: &QueueEntry,
    versions: &LyricsVersions,
) -> Result<(), String> {
    let origin = sound::recording(&entry.playlist, &entry.track);
    let requests = requests(store, entry, origin, Some(Source::Lrclib))?;
    let key = requests
        .first()
        .and_then(|request| request.cache_keys(&Recording::from_entry(entry))[0].clone())
        .ok_or("Не удалось построить ключ тестового текста.")?;
    store
        .save_lyrics_snapshot(&key, None, http_cache::now(), 600, versions)
        .map(|_| ())
        .map_err(display)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Current,
    Retry,
    Upcoming,
}

fn load_requested(
    store: &Store,
    entry: &QueueEntry,
    sound: Option<&SoundSource>,
    manual_source: Option<Source>,
    cancelled: &AtomicBool,
    purpose: Purpose,
    mut emit: impl FnMut(LyricsVersions),
) -> Result<(), String> {
    if manual_source == Some(Source::Genius) || cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let recording = Recording::from_entry(entry);
    let origin = sound::recording(&entry.playlist, &entry.track);
    let sounding_provider = sound
        .and_then(|sound| sound.provider())
        .or_else(|| origin.as_ref().map(|reference| reference.provider));
    let requests = requests(store, entry, origin, manual_source)?;
    let now = http_cache::now();
    store.purge_expired_lyrics(now).map_err(display)?;
    let mut selection = Selection::default();
    let mut outstanding = Vec::with_capacity(requests.len());
    'request: for request in requests {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let [key, original] = request.cache_keys(&recording);
        for lookup in [key.as_deref(), original.as_deref()].into_iter().flatten() {
            if let Some(snapshot) = store
                .lyrics_snapshot(lookup, request.account(), now)
                .map_err(display)?
            {
                let fresh = snapshot.fresh && !snapshot.versions.needs_format_revalidation();
                selection.consider(snapshot.versions, sounding_provider, &mut emit);
                if fresh {
                    continue 'request;
                }
                break;
            }
        }
        outstanding.push((request, key));
    }

    let mut error = None;
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for (request, key) in outstanding {
            let sender = sender.clone();
            let recording = &recording;
            scope.spawn(move || {
                let source = request.source();
                let result = refresh(
                    store,
                    &request,
                    key.as_deref(),
                    recording,
                    cancelled,
                    purpose,
                );
                let _ = sender.send((source, result));
            });
        }
        drop(sender);
        for (source, result) in receiver {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            match result {
                Ok(Some(lyrics)) => {
                    selection.consider(Arc::unwrap_or_clone(lyrics), sounding_provider, &mut emit);
                }
                Ok(None) => {}
                Err(message) if error.as_ref().is_none_or(|(other, _)| source < *other) => {
                    error = Some((source, message));
                }
                Err(_) => {}
            }
        }
    });
    if !cancelled.load(Ordering::Relaxed)
        && selection.best.is_none()
        && let Some((_, message)) = error
    {
        return Err(message);
    }
    Ok(())
}

fn refresh(
    store: &Store,
    request: &Request,
    key: Option<&str>,
    recording: &Recording,
    cancelled: &AtomicBool,
    purpose: Purpose,
) -> Result<Option<Arc<LyricsVersions>>, String> {
    let pending_key = request.pending_key(store, key);
    let local_revision = match pending_key.as_ref() {
        Some(Key::Local(_, revision)) => *revision,
        _ => None,
    };
    let work = |foreground: &AtomicBool| {
        if let Some(key) = key
            && let Some(snapshot) = store
                .lyrics_snapshot(key, request.account(), http_cache::now())
                .map_err(display)?
            && snapshot.fresh
            && !snapshot.versions.needs_format_revalidation()
        {
            return Ok(Loaded {
                text: Some(Arc::new(snapshot.versions)),
                retain_for: None,
            });
        }
        let found = match request {
            Request::Local(_) => request.fetch(store, recording, cancelled),
            _ => {
                let Some(_slot) = NETWORK.acquire(request.source(), foreground, cancelled) else {
                    return Ok(Loaded {
                        text: None,
                        retain_for: None,
                    });
                };
                request.fetch(store, recording, cancelled)
            }
        }?;
        let (mut versions, max_age) = match found {
            Lookup::Found(versions, max_age) if versions.preferred().usable() => {
                (versions, max_age)
            }
            Lookup::Missing(age) => {
                let retain_for = age
                    .and_then(|age| u64::try_from(age).ok())
                    .map(|seconds| Duration::from_secs(seconds).min(ABSENCE_RETENTION));
                return Ok(Loaded {
                    text: None,
                    retain_for,
                });
            }
            _ => {
                return Ok(Loaded {
                    text: None,
                    retain_for: None,
                });
            }
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Loaded {
                text: None,
                retain_for: None,
            });
        }
        versions.preferred.prepare();
        let available = match key.zip(max_age) {
            Some((key, age)) => store
                .save_lyrics_snapshot(key, request.account(), http_cache::now(), age, &versions)
                .map_err(display)?,
            None => true,
        };
        let retain_for = match (request, local_revision) {
            (Request::Local(path), Some(revision)) if local::revision(path) == Some(revision) => {
                Some(LOCAL_RETENTION)
            }
            _ => None,
        };
        Ok(Loaded {
            text: available.then(|| Arc::new(versions)),
            retain_for,
        })
    };
    let foreground = purpose != Purpose::Upcoming;
    let loaded = match pending_key {
        Some(key) => pending::read(key, cancelled, foreground, purpose == Purpose::Retry, work),
        None => work(&AtomicBool::new(foreground)).map(|loaded| loaded.text),
    };
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    // A private fallback must never be returned through the shared public flight.
    let loaded = match loaded {
        Err(message) => request
            .legacy_lyrics(store, recording, key)?
            .ok_or(message)
            .map(Some)?,
        Ok(loaded) => loaded,
    };
    if let Some(account) = request.account()
        && !store.lyrics_account_is_current(account).map_err(display)?
    {
        return Ok(None);
    }
    Ok(loaded.filter(|_| !cancelled.load(Ordering::Relaxed)))
}

fn requests(
    store: &Store,
    entry: &QueueEntry,
    origin: Option<ObjectRef>,
    manual: Option<Source>,
) -> Result<Vec<Request>, String> {
    let wanted = |source| manual.is_none_or(|selected| selected == source);
    let mut requests = Vec::new();
    if wanted(Source::Local)
        && let Some(Media::File(path)) = &entry.track.media
    {
        requests.push(Request::Local(path.clone()));
    }
    if wanted(Source::Lrclib) {
        let youtube_title = origin
            .as_ref()
            .filter(|origin| origin.provider == ProviderId::YouTubeMusic)
            .and_then(|_| youtube_song_title(&entry.track.title, &entry.track.artist))
            .map(str::to_owned);
        requests.push(Request::Lrclib { youtube_title });
    }
    let Some(origin) = origin else {
        return Ok(requests);
    };
    if !NATIVE_SOURCES.into_iter().any(wanted) {
        return Ok(requests);
    }
    let region = region();
    let mut confirmed = cached_links(store, &region, &origin).map_err(display)?;
    confirmed.sort_by(|a, b| a.url.cmp(&b.url));
    for source in NATIVE_SOURCES.into_iter().filter(|source| wanted(*source)) {
        let provider = source.provider().expect("native lyrics provider");
        let same_provider = origin.provider == provider;
        let reference = if same_provider {
            Some(origin.clone())
        } else {
            confirmed_reference(store, &region, provider, &confirmed)?
        };
        let Some(reference) = reference else { continue };
        if source == Source::YouTubeMusic {
            let legacy_account = match entry.playlist.connected_source() {
                Some((ProviderId::YouTubeMusic, Some(profile))) => store
                    .account(ProviderId::YouTubeMusic, profile)
                    .map_err(display)?,
                _ => None,
            };
            requests.push(Request::PublicYouTube {
                id: reference.id,
                legacy_account,
            });
            continue;
        }
        let account = match entry.playlist.connected_source() {
            Some((playlist_provider, Some(profile))) if playlist_provider == provider => {
                store.account(provider, profile)
            }
            _ => store.preferred_account(provider),
        }
        .map_err(display)?;
        if let Some(account) = account {
            requests.push(Request::Native {
                source,
                reference,
                account,
            });
        }
    }
    Ok(requests)
}

fn confirmed_reference(
    store: &Store,
    region: &str,
    provider: ProviderId,
    links: &[crate::music::ConfirmedLink],
) -> Result<Option<ObjectRef>, String> {
    let mut references = Vec::new();
    for link in links {
        let Some(public) = links::public_link(&link.url) else {
            continue;
        };
        let valid = |item: &ObjectRef| item.kind == ObjectKind::Track && item.provider == provider;
        let reference = store
            .native_music_alias(region, &public)
            .map_err(display)?
            .filter(valid)
            .or_else(|| ObjectRef::from_url(&link.url).filter(valid));
        if let Some(reference) = reference
            && !references.contains(&reference)
        {
            references.push(reference);
        }
    }
    Ok((references.len() == 1).then(|| references.remove(0)))
}

type Rank = (Timing, bool, bool, Reverse<Source>);

#[derive(Default)]
struct Selection {
    best: Option<(Rank, LyricsVersions)>,
}

impl Selection {
    fn consider(
        &mut self,
        versions: LyricsVersions,
        sounding: Option<ProviderId>,
        emit: &mut impl FnMut(LyricsVersions),
    ) {
        let lyrics = versions.preferred();
        let source = lyrics.source;
        if versions.needs_format_revalidation()
            && self.best.as_ref().is_some_and(|(_, previous)| {
                previous.preferred().source == source && previous.alternate_plain().is_some()
            })
        {
            return;
        }
        let rank = (
            lyrics.timing(),
            sounding.is_some() && source.provider() == sounding,
            source == Source::Local,
            Reverse(source),
        );
        if self
            .best
            .as_ref()
            .is_none_or(|(old, previous)| rank >= *old && versions != *previous)
        {
            emit(versions.clone());
            self.best = Some((rank, versions));
        }
    }
}

fn display(error: crate::storage::StoreError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::lyrics::{Lyrics, parse_lrc};
    use crate::playlist::{PlaylistKey, PlaylistTrack};

    #[test]
    fn temporary_yandex_format_failure_preserves_a_previous_pair() {
        let mut selection = Selection::default();
        let source = Source::YandexMusic;
        let timed = parse_lrc(source, "[00:01.00]The older synchronized line").unwrap();
        let plain = Lyrics::plain(source, "Second provided format");
        let pair = LyricsVersions::from_source(timed, Some(plain));
        let refreshed = parse_lrc(source, "[00:01.00]Temporary single format").unwrap();
        let mut emitted = Vec::new();
        let mut emit = |versions| emitted.push(versions);
        selection.consider(pair.clone(), Some(ProviderId::YandexMusic), &mut emit);
        selection.consider(refreshed.into(), Some(ProviderId::YandexMusic), &mut emit);
        assert_eq!(emitted, vec![pair]);
    }

    #[test]
    fn youtube_lyrics_cache_reads_old_hits_and_scopes_short_title_hits() {
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(profile.path()).unwrap();
        let provider = ProviderId::YouTubeMusic;
        let mut entry = QueueEntry {
            playlist: PlaylistKey::Source {
                provider,
                profile: None,
                id: "liked".into(),
            },
            playlist_revision: None,
            index: 0,
            recommendations: Vec::new(),
            track: PlaylistTrack {
                id: "abcdefghijk".into(),
                title: "Artist - Track".into(),
                artist: "Artist".into(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album_ref: None,
                album: None,
                artwork: None,
                added: None,
                duration: Some(150),
                media: Some(Media::Provider {
                    provider,
                    id: "abcdefghijk".into(),
                }),
            },
        };
        let recording = Recording::from_entry(&entry);
        let origin = sound::recording(&entry.playlist, &entry.track);
        let requests = requests(&store, &entry, origin, Some(Source::Lrclib)).unwrap();
        let [youtube, original] = requests[0].cache_keys(&recording);
        let youtube = youtube.unwrap();
        let original = original.unwrap();
        assert_ne!(youtube, original);
        let save = |key: &str, words| {
            let lyrics = Lyrics::plain(Source::Lrclib, words);
            assert!(
                store
                    .save_lyrics_snapshot(
                        key,
                        None,
                        http_cache::now(),
                        600,
                        &LyricsVersions::from_source(lyrics, None),
                    )
                    .unwrap()
            );
        };
        let load_cached = |entry: &QueueEntry| {
            let mut result = None;
            load(
                &store,
                entry,
                None,
                Some(Source::Lrclib),
                &AtomicBool::new(false),
                |lyrics| result = Some(lyrics),
            )
            .unwrap();
            result.unwrap().preferred().lines[0].text.clone()
        };
        save(&original, "Literal search");
        assert_eq!(load_cached(&entry), "Literal search");
        save(&youtube, "Short title search");
        assert_eq!(load_cached(&entry), "Short title search");
        entry.track.media = None;
        entry.playlist = PlaylistKey::Local(1);
        assert_eq!(load_cached(&entry), "Literal search");
    }

    #[test]
    fn word_timing_wins_and_native_breaks_equal_ties() {
        let mut selection = Selection::default();
        let mut reported = Vec::new();
        let mut emit = |versions: LyricsVersions| reported.push(versions.preferred().source);
        let sounding = Some(ProviderId::YandexMusic);
        selection.consider(
            parse_lrc(Source::Lrclib, "[00:02]Public").unwrap().into(),
            sounding,
            &mut emit,
        );
        selection.consider(
            parse_lrc(Source::YandexMusic, "[00:03]Native")
                .unwrap()
                .into(),
            sounding,
            &mut emit,
        );
        selection.consider(
            parse_lrc(Source::Local, "[00:03]<00:03>Words<00:04>")
                .unwrap()
                .into(),
            sounding,
            &mut emit,
        );
        selection.consider(
            parse_lrc(Source::YandexMusic, "[00:04]Native")
                .unwrap()
                .into(),
            sounding,
            &mut emit,
        );
        assert_eq!(
            reported,
            vec![Source::Lrclib, Source::YandexMusic, Source::Local]
        );
    }
}
