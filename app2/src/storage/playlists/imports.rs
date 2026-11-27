mod cache;

use std::{
    borrow::Cow,
    collections::{HashMap, hash_map::Entry},
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use lofty::{
    config::ParseOptions,
    file::{AudioFile as _, TaggedFileExt as _},
    probe::Probe,
    tag::Accessor as _,
};
use rusqlite::{Connection, params};

use super::{Result, StoreError};

const M3U_BYTES_LIMIT: u64 = 8 * 1024 * 1024;
const BATCH_SIZE: usize = 64;
const PREVIEW_TITLES: usize = 3;
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "flac", "m4a", "mka", "mp3", "ogg", "opus", "wav", "wma",
];

#[derive(Clone)]
struct Track {
    path: String,
    title: String,
    artist: String,
    album: Option<String>,
    duration: Option<u32>,
}

pub(super) const TAG_CACHE_MIGRATION: &str = cache::MIGRATION;

#[derive(Clone, Default)]
struct Metadata {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration: Option<u32>,
}

#[derive(Debug)]
pub(crate) enum ImportProgress {
    Discovering {
        found: usize,
    },
    Reading {
        processed: usize,
        reused: usize,
        unreadable: usize,
        titles: Vec<String>,
    },
    Appending {
        count: usize,
        titles: Vec<String>,
    },
}

#[derive(Default)]
pub(super) struct Staged {
    pub(super) count: usize,
    pub(super) preview: Vec<String>,
}

pub(super) fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(StoreError::ImportCancelled);
    }
    Ok(())
}

pub(super) fn stage_tracks(
    connection: &mut Connection,
    files: &[PathBuf],
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<Staged> {
    if files.is_empty() {
        return Ok(Staged::default());
    }
    // This connection owns the staged occurrences until publication or cancellation.
    connection.execute_batch(
        "CREATE TEMP TABLE import_track AS
         SELECT ordinal, path, title, artist, album, duration FROM local_playlist_track WHERE 0",
    )?;
    let mut scan = Scan {
        connection,
        cancelled,
        progress,
        batch: Vec::with_capacity(BATCH_SIZE),
        cache_writes: Vec::with_capacity(BATCH_SIZE),
        processed: 0,
        reused: 0,
        unreadable: 0,
        preview: Vec::with_capacity(PREVIEW_TITLES),
    };
    for file in files {
        check_cancelled(cancelled)?;
        if file.is_dir() {
            for path in scan.folder_audio(file)? {
                check_cancelled(cancelled)?;
                let track = scan.read_track(&path, false)?;
                scan.push(track, &path, None)?;
            }
        } else if is_m3u_path(file) {
            import_m3u(file, &mut scan)?;
        } else {
            let track = scan.read_track(file, false)?;
            scan.push(track, file, None)?;
        }
    }
    scan.flush()?;
    Ok(Staged {
        count: scan.processed,
        preview: scan.preview,
    })
}

struct Scan<'a> {
    connection: &'a mut Connection,
    cancelled: &'a AtomicBool,
    progress: &'a mut dyn FnMut(ImportProgress),
    batch: Vec<Track>,
    cache_writes: Vec<cache::Record>,
    processed: usize,
    reused: usize,
    unreadable: usize,
    preview: Vec<String>,
}

impl Scan<'_> {
    fn folder_audio(&mut self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut directories = vec![root.to_path_buf()];
        while let Some(directory) = directories.pop() {
            check_cancelled(self.cancelled)?;
            for entry in fs::read_dir(directory)? {
                check_cancelled(self.cancelled)?;
                let entry = entry?;
                let kind = entry.file_type()?;
                let path = entry.path();
                if kind.is_dir() {
                    directories.push(path);
                } else if kind.is_file() && has_extension(&path, AUDIO_EXTENSIONS) {
                    files.push(path);
                    if files.len().is_multiple_of(BATCH_SIZE) {
                        (self.progress)(ImportProgress::Discovering { found: files.len() });
                    }
                }
            }
        }
        if files.is_empty() {
            return Err(input_error(root, "в папке нет аудиофайлов"));
        }
        files.sort_unstable();
        Ok(files)
    }

    fn read_track(&mut self, path: &Path, allow_missing: bool) -> Result<Track> {
        if !has_extension(path, AUDIO_EXTENSIONS) {
            return Err(input_error(path, "неподдерживаемый формат аудио"));
        }
        let (absolute, file) = match fs::canonicalize(path) {
            Ok(absolute) => {
                let file = fs::metadata(&absolute)?;
                if !file.is_file() {
                    return Err(input_error(path, "ожидается аудиофайл"));
                }
                (absolute, Some(file))
            }
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
                // Keep the original occurrence so playback can retry after the file returns.
                (std::path::absolute(path)?, None)
            }
            Err(error) => return Err(input_error(path, &error.to_string())),
        };
        let display = display_path(&absolute);
        let normalized = display
            .to_str()
            .ok_or_else(|| input_error(path, "путь не представим в UTF-8"))?;
        let metadata = match &file {
            Some(file) => self.tags(&absolute, normalized, file)?,
            None => Metadata::default(),
        };
        Ok(Track {
            path: normalized.to_owned(),
            title: metadata.title.unwrap_or_default(),
            artist: metadata.artist.unwrap_or_default(),
            album: metadata.album,
            duration: metadata.duration,
        })
    }

    fn tags(&mut self, path: &Path, key: &str, file: &fs::Metadata) -> Result<Metadata> {
        let stamp = cache::FileStamp::read(file);
        if let Some(stamp) = &stamp
            && let Some(metadata) = cache::lookup(self.connection, key, stamp)?
        {
            self.reused += 1;
            return Ok(metadata);
        }
        let Some(tags) = probe(path) else {
            // Files without readable tags still open through the player.
            self.unreadable += 1;
            return Ok(Metadata::default());
        };
        if let Some(stamp) = stamp
            && fs::metadata(path)
                .ok()
                .and_then(|file| cache::FileStamp::read(&file))
                .as_ref()
                == Some(&stamp)
        {
            self.cache_writes.push(cache::Record {
                path: key.to_owned(),
                stamp,
                tags: tags.clone(),
            });
        }
        Ok(tags)
    }

    fn push(&mut self, mut track: Track, path: &Path, history: Option<Metadata>) -> Result<()> {
        if let Some(history) = history {
            track.duration = track.duration.or(history.duration);
            if track.title.is_empty() {
                track.title = history.title.unwrap_or_default();
            }
        }
        if track.title.is_empty() {
            track.title = path
                .file_stem()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
        }
        self.batch.push(track);
        if self.batch.len() == BATCH_SIZE {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        check_cancelled(self.cancelled)?;
        if self.batch.is_empty() {
            return Ok(());
        }
        let transaction = self.connection.transaction()?;
        cache::write(&transaction, &self.cache_writes)?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO temp.import_track (ordinal, path, title, artist, album, duration)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (index, track) in self.batch.iter().enumerate() {
                check_cancelled(self.cancelled)?;
                let ordinal =
                    i64::try_from(self.processed + index).map_err(|_| StoreError::InvalidData)?;
                insert.execute(params![
                    ordinal,
                    track.path,
                    track.title,
                    track.artist,
                    track.album,
                    track.duration
                ])?;
            }
        }
        transaction.commit()?;
        self.cache_writes.clear();
        self.preview.extend(
            self.batch
                .iter()
                .take(PREVIEW_TITLES - self.preview.len())
                .map(|track| track.title.clone()),
        );
        self.processed += self.batch.len();
        let preview_start = self.batch.len().saturating_sub(PREVIEW_TITLES);
        (self.progress)(ImportProgress::Reading {
            processed: self.processed,
            reused: self.reused,
            unreadable: self.unreadable,
            titles: self.batch[preview_start..]
                .iter()
                .map(|track| track.title.clone())
                .collect(),
        });
        self.batch.clear();
        Ok(())
    }
}

fn import_m3u(playlist: &Path, scan: &mut Scan<'_>) -> Result<()> {
    let absolute = fs::canonicalize(playlist)?;
    let playlist = display_path(&absolute);
    let parent = playlist.parent().ok_or(StoreError::InvalidData)?;
    let mut bytes = Vec::new();
    File::open(&absolute)?
        .take(M3U_BYTES_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > M3U_BYTES_LIMIT {
        return Err(input_error(&playlist, "файл M3U превышает 8 МБ"));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| input_error(&playlist, "требуется список M3U в кодировке UTF-8"))?;
    let text = text.trim_start_matches('\u{feff}');
    if text
        .lines()
        .any(|line| line.trim_start().starts_with("#EXT-X-"))
    {
        return Err(input_error(
            &playlist,
            "потоковый манифест HLS не является локальным плейлистом",
        ));
    }
    let mut metadata = Metadata::default();
    let mut loaded = HashMap::<PathBuf, Track>::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        check_cancelled(scan.cancelled)?;
        if let Some(extinf) = line.strip_prefix("#EXTINF:")
            && let Some((duration, title)) = extinf.split_once(',')
        {
            metadata.duration = duration
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|duration| *duration > 0);
            metadata.title = (!title.trim().is_empty()).then(|| title.trim().to_owned());
        }
        if line.starts_with('#') {
            continue;
        }
        let path = if line.starts_with("file://") {
            url::Url::parse(line)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .ok_or_else(|| input_error(&playlist, "некорректная ссылка на локальный файл"))?
        } else if line.contains("://") {
            return Err(input_error(
                &playlist,
                "поддерживаются ссылки только на локальные аудиофайлы",
            ));
        } else {
            parent.join(line)
        };
        let track = match loaded.entry(path.clone()) {
            Entry::Occupied(track) => track.get().clone(),
            Entry::Vacant(entry) => entry.insert(scan.read_track(&path, true)?).clone(),
        };
        scan.push(track, &path, Some(std::mem::take(&mut metadata)))?;
    }
    Ok(())
}

fn probe(path: &Path) -> Option<Metadata> {
    let file = Probe::open(path)
        .map(|probe| probe.options(ParseOptions::new().read_cover_art(false)))
        .and_then(Probe::read)
        .ok()?;
    let tags = file.primary_tag().into_iter().chain(file.tags());
    let duration = u32::try_from(file.properties().duration().as_secs())
        .ok()
        .filter(|seconds| *seconds > 0);
    Some(Metadata {
        title: tags.clone().find_map(|tag| nonempty(tag.title())),
        artist: tags.clone().find_map(|tag| nonempty(tag.artist())),
        album: tags.clone().find_map(|tag| nonempty(tag.album())),
        duration,
    })
}

fn nonempty(value: Option<Cow<'_, str>>) -> Option<String> {
    let value = value?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(crate) fn is_m3u_path(path: &Path) -> bool {
    has_extension(path, &["m3u", "m3u8"])
}

fn has_extension(path: &Path, choices: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            choices
                .iter()
                .any(|choice| choice.eq_ignore_ascii_case(extension))
        })
}

fn display_path(path: &Path) -> Cow<'_, Path> {
    if let Some(unc) = path
        .to_str()
        .and_then(|path| path.strip_prefix(r"\\?\UNC\"))
    {
        Cow::Owned(PathBuf::from(format!(r"\\{unc}")))
    } else if let Some(drive) = path.to_str().and_then(|path| path.strip_prefix(r"\\?\"))
        && drive.as_bytes().get(1) == Some(&b':')
    {
        Cow::Borrowed(Path::new(drive))
    } else {
        Cow::Borrowed(path)
    }
}

fn input_error(path: &Path, issue: &str) -> StoreError {
    let name = path.file_name().unwrap_or(path.as_os_str());
    StoreError::PlaylistInput(format!("{} — {issue}.", name.to_string_lossy()))
}
