use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    music::shared_reads::{SharedReads, check_cancelled},
    providers::{ErrorKind, ProviderError, genius as api},
    storage::{GeniusSession, GeniusSnapshot, GeniusSnapshotKind as Kind, Store},
};

use super::{Lyrics, Recording, Source};

pub(crate) use api::Annotation;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Credits {
    pub writers: Vec<String>,
    pub producers: Vec<String>,
}

#[derive(Clone, Eq, PartialEq)]
struct DetailScope {
    profile: PathBuf,
    epoch: i64,
    recording: String,
}

static DETAILS: SharedReads<DetailScope, Option<api::Details>> = SharedReads::new();

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i64)]
pub(crate) enum Access {
    #[default]
    Link = 0,
    Read = 1,
    Save = 2,
}

#[derive(Clone, Debug)]
pub(crate) struct Song {
    pub url: Option<String>,
    pub title: String,
    pub artist: String,
    pub writers: Vec<String>,
    pub producers: Vec<String>,
    pub annotations: Vec<Annotation>,
    pub lyrics: Option<Lyrics>,
    pub access: Access,
    pub revoked: bool,
    pub issue: Option<String>,
    epoch: i64,
}

impl Song {
    pub(crate) fn is_current(&self, store: &Store) -> bool {
        store.genius_epoch_current(self.epoch).unwrap_or(false)
    }
}

pub(crate) fn set_access(store: &Store, song: &Song, access: Access) -> Result<(), String> {
    let url = song
        .url
        .as_deref()
        .and_then(api::song_url)
        .ok_or("Genius не вернул ссылку на песню.")?;
    store
        .set_genius_access(song.epoch, url.as_str(), access as i64)
        .map_err(|error| format!("Не удалось изменить разрешение Genius. {error}"))
}

pub(crate) fn connect(store: &Store, token: &str) -> Result<(), String> {
    let token = token.trim();
    api::validate_token(token, &AtomicBool::new(false))?;
    store.set_genius_token(Some(token)).map_err(store_error)
}

pub(crate) fn load(
    store: &Store,
    recording: &Recording,
    cancelled: &AtomicBool,
    mut publish_cached: impl FnMut(Song),
) -> Result<Option<Song>, String> {
    let Some(request) = Request::open(store, recording, cancelled)? else {
        return Ok(None);
    };
    store.prune_genius_snapshots().map_err(store_error)?;
    if !request.current() {
        return Ok(None);
    }
    let cached = request.snapshot::<api::Details>(Kind::Details)?;
    let fresh = cached.as_ref().is_some_and(|value| value.fresh);
    let mut prepared = cached
        .as_ref()
        .map(|details| request.prepare(&details.value))
        .transpose()?;
    if let Some(cached) = &prepared {
        if !request.current() {
            return Ok(None);
        }
        if fresh && !cached.need_annotations && !cached.need_lyrics {
            return Ok(Some(cached.song.clone()));
        }
        publish_cached(cached.song.clone());
    }
    let details = if let Some(details) = cached.filter(|value| value.fresh) {
        details.value
    } else {
        match request.fetch_details(recording) {
            Ok(Some(fetched)) if request.current() => {
                prepared = None;
                fetched
            }
            Ok(_) => return Ok(None),
            Err(error) if request.current() => {
                let Some(mut cached) = prepared else {
                    return Err(error);
                };
                cached.song.issue = Some(error);
                return Ok(Some(cached.song));
            }
            Err(_) => return Ok(None),
        }
    };
    if !request.current() {
        return Ok(None);
    }
    let mut result = match prepared {
        Some(cached) => cached,
        None => request.prepare(&details)?,
    };
    if result.need_annotations && request.current() {
        match api::annotations(&request.session.token, details.id, cancelled) {
            Ok(fetched) if request.current() => {
                request.save(
                    Kind::Annotations,
                    &details.url,
                    &fetched.value,
                    fetched.max_age,
                );
                result.song.annotations = fetched.value;
            }
            Ok(_) => return Ok(None),
            Err(error) => result.song.issue = Some(error),
        }
    }
    if result.need_lyrics && request.current() {
        match api::lyric_page(&details.url, cancelled) {
            Ok(fetched) if request.current() => {
                if let Some(text) = fetched.value {
                    let mut lyrics = Lyrics::plain(Source::Genius, &text);
                    lyrics.attribution = Some(details.url.clone());
                    if result.song.access == Access::Save {
                        request.save(Kind::Lyrics, &details.url, &lyrics, fetched.max_age);
                    }
                    result.song.lyrics = Some(lyrics);
                } else {
                    result.song.issue = Some("Genius не вернул текст песни.".into());
                }
            }
            Ok(_) => return Ok(None),
            Err(error) => result.song.issue = Some(error),
        }
    }
    Ok(request.current().then_some(result.song))
}

pub(crate) fn load_credits(
    store: &Store,
    recording: &Recording,
    cancelled: &AtomicBool,
) -> Result<Option<Credits>, String> {
    let Some(request) = Request::open(store, recording, cancelled)? else {
        return Ok(None);
    };
    let cached = request.snapshot::<api::Details>(Kind::Details)?;
    let details = if let Some(value) = cached.as_ref().filter(|value| value.fresh) {
        Some(value.value.clone())
    } else {
        match request.fetch_details(recording) {
            Ok(details) => details,
            Err(error) if request.current() => Some(cached.ok_or(error)?.value),
            Err(_) => return Ok(None),
        }
    };
    Ok(details
        .filter(|_| request.current())
        .map(|details| Credits {
            writers: details.writers,
            producers: details.producers,
        }))
}

struct Prepared {
    song: Song,
    need_annotations: bool,
    need_lyrics: bool,
}

struct Request<'a> {
    store: &'a Store,
    session: GeniusSession,
    key: String,
    cancelled: &'a AtomicBool,
}

impl<'a> Request<'a> {
    fn open(
        store: &'a Store,
        recording: &Recording,
        cancelled: &'a AtomicBool,
    ) -> Result<Option<Self>, String> {
        if cancelled.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Some(session) = store.genius_session().map_err(store_error)? else {
            return Ok(None);
        };
        let key = serde_json::to_string(recording).map_err(|error| error.to_string())?;
        Ok(Some(Self {
            store,
            session,
            key,
            cancelled,
        }))
    }

    fn fetch_details(&self, recording: &Recording) -> Result<Option<api::Details>, String> {
        let scope = DetailScope {
            profile: self.store.directory().to_owned(),
            epoch: self.session.epoch,
            recording: self.key.clone(),
        };
        let unavailable = |message| ProviderError {
            kind: ErrorKind::Unavailable,
            message,
        };
        DETAILS
            .read(&scope, self.cancelled, || {
                check_cancelled(self.cancelled)?;
                if !self.current() {
                    return Ok(None);
                }
                if let Some(cached) = self
                    .snapshot::<api::Details>(Kind::Details)
                    .map_err(unavailable)?
                    && cached.fresh
                {
                    return Ok(Some(cached.value));
                }
                let details = api::details(&self.session.token, recording, self.cancelled);
                check_cancelled(self.cancelled)?;
                if !self.current() {
                    return Ok(None);
                }
                Ok(details.map_err(unavailable)?.map(|fetched| {
                    self.save(
                        Kind::Details,
                        &fetched.value.url,
                        &fetched.value,
                        fetched.max_age,
                    );
                    fetched.value
                }))
            })
            .map_err(|error| error.message)
    }

    fn current(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
            && self
                .store
                .genius_epoch_current(self.session.epoch)
                .unwrap_or(false)
    }

    fn snapshot<T: serde::de::DeserializeOwned>(
        &self,
        kind: Kind,
    ) -> Result<Option<GeniusSnapshot<T>>, String> {
        self.store
            .genius_snapshot(self.session.epoch, &self.key, kind)
            .map_err(store_error)
    }

    fn save(&self, kind: Kind, url: &str, value: &impl serde::Serialize, max_age: Option<i64>) {
        if let Err(error) = self.store.save_genius_snapshot(
            self.session.epoch,
            &self.key,
            kind,
            url,
            value,
            max_age,
        ) {
            eprintln!("Не удалось сохранить снимок Genius: {error}");
        }
    }

    fn prepare(&self, details: &api::Details) -> Result<Prepared, String> {
        let grant = self
            .store
            .genius_access(self.session.epoch, &details.url)
            .map_err(store_error)?;
        let access = match grant {
            Some(1) => Access::Read,
            Some(2) => Access::Save,
            _ => Access::Link,
        };
        let revoked = grant == Some(Access::Link as i64);
        let annotations = if revoked {
            None
        } else {
            self.snapshot::<Vec<Annotation>>(Kind::Annotations)?
                .filter(|value| value.url == details.url)
        };
        let lyrics = if access == Access::Save {
            self.snapshot::<Lyrics>(Kind::Lyrics)?
                .filter(|value| value.url == details.url)
        } else {
            None
        };
        let need_annotations = !revoked && !annotations.as_ref().is_some_and(|value| value.fresh);
        let need_lyrics =
            access != Access::Link && !lyrics.as_ref().is_some_and(|value| value.fresh);
        Ok(Prepared {
            need_annotations,
            need_lyrics,
            song: Song {
                url: Some(details.url.clone()),
                title: details.title.clone(),
                artist: details.artist.clone(),
                writers: details.writers.clone(),
                producers: details.producers.clone(),
                annotations: annotations.map(|value| value.value).unwrap_or_default(),
                lyrics: lyrics.map(|value| value.value),
                access,
                revoked,
                issue: None,
                epoch: self.session.epoch,
            },
        })
    }
}

fn store_error(error: crate::storage::StoreError) -> String {
    format!("Не удалось открыть данные Genius. {error}")
}
