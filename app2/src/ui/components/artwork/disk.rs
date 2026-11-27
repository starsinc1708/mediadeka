use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

use gpui::{Image, ImageFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    providers::{ProviderId, artwork_provider},
    storage::Store,
};

use super::{RESPONSE_LIMIT, http::Validators};

const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub(super) const FRESH_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_FILES: usize = 160;
pub(super) const MAX_BLOBS: usize = MAX_FILES * ProviderId::ALL.len();
const MAX_BYTES: u64 = 96 * 1024 * 1024;
const ARTWORK_DIRECTORY: &str = "artwork";
const BLOB_DIRECTORY: &str = "blobs";
const GENERATION_FILE: &str = "cache-generation";
const TEMPORARY_EXTENSION: &str = "pending";
const CONTENT_REFERENCE: u8 = 4;
const VALIDATED_REFERENCE: u8 = 5;
const REFERENCE_METADATA_LIMIT: u64 = 1024;

pub(super) type ContentId = [u8; 32];
const REFERENCE_LENGTH: u64 = (1 + size_of::<ContentId>()) as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum Rendition {
    Larger,
    Original,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct SourceRecord {
    pub(super) rendition: Rendition,
    pub(super) validators: Validators,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Receipt {
    pub(super) content: ContentId,
    pub(super) source: Option<SourceRecord>,
    checked_at: SystemTime,
}

impl Receipt {
    pub(super) fn fresh_for(&self) -> Duration {
        FRESH_AGE.saturating_sub(self.checked_at.elapsed().unwrap_or_default())
    }
}

pub(super) struct EntryImage {
    pub(super) image: Image,
    pub(super) receipt: Receipt,
}

pub(super) fn path(profile_directory: &Path, url: &str, larger: Option<&str>) -> Option<PathBuf> {
    let provider = artwork_provider(url)?;
    let mut hash = Sha256::new();
    hash.update(url.as_bytes());
    if let Some(larger) = larger {
        if artwork_provider(larger) != Some(provider) {
            return None;
        }
        hash.update(b"\0");
        hash.update(larger.as_bytes());
    }
    let directory = profile_directory
        .join(ARTWORK_DIRECTORY)
        .join(provider.as_str());
    Some(directory.join(format!("{:x}.cover", hash.finalize())))
}

#[cfg(any(test, feature = "capture"))]
pub(super) fn read(path: &Path) -> Option<Image> {
    read_entry(path).map(|entry| entry.image)
}

pub(super) fn read_entry(path: &Path) -> Option<EntryImage> {
    read_entry_with_age(path, false)
}

fn read_entry_with_age(path: &Path, permit_expired: bool) -> Option<EntryImage> {
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let checked_at = metadata.modified().ok()?;
    if !permit_expired && checked_at.elapsed().ok()? >= MAX_AGE {
        return None;
    }
    let tag = read_tag(&mut file)?;
    if matches!(tag, CONTENT_REFERENCE | VALIDATED_REFERENCE) {
        let length = metadata.len();
        if tag == CONTENT_REFERENCE && length != REFERENCE_LENGTH
            || tag == VALIDATED_REFERENCE
                && !(REFERENCE_LENGTH..=REFERENCE_LENGTH + REFERENCE_METADATA_LIMIT)
                    .contains(&length)
        {
            return None;
        }
        let mut content = ContentId::default();
        file.read_exact(&mut content).ok()?;
        let source = if tag == VALIDATED_REFERENCE {
            let mut bytes = Vec::with_capacity((length - REFERENCE_LENGTH) as usize);
            file.take(REFERENCE_METADATA_LIMIT)
                .read_to_end(&mut bytes)
                .ok()?;
            serde_json::from_slice(&bytes).ok()
        } else {
            None
        };
        let image = read_blob(cache_root(path)?, &content)?;
        return Some(EntryImage {
            image,
            receipt: Receipt {
                content,
                source,
                checked_at,
            },
        });
    }
    // Older profiles and acceptance fixtures carry their image inline.
    let format = image_format(tag)?;
    let bytes = read_payload(file, metadata.len())?;
    let content = content_id(tag, &bytes);
    Some(EntryImage {
        image: Image::from_bytes(format, bytes),
        receipt: Receipt {
            content,
            source: None,
            checked_at,
        },
    })
}

#[cfg(any(test, feature = "capture"))]
pub(super) fn save(path: &Path, image: &Image) {
    if let Some(generation) = generation(path) {
        save_current(path, image, generation, &AtomicBool::new(false));
    }
}

pub(super) fn generation(path: &Path) -> Option<u64> {
    read_generation(cache_root(path)?).ok()
}

#[cfg(any(test, feature = "capture"))]
pub(super) fn save_current(path: &Path, image: &Image, generation: u64, cancelled: &AtomicBool) {
    let Some(root) = cache_root(path) else {
        return;
    };
    let Some(tag) = format_tag(image) else {
        return;
    };
    if cancelled.load(Ordering::Acquire)
        || !(1..=RESPONSE_LIMIT).contains(&(image.bytes.len() as u64))
    {
        return;
    }
    let _ = with_current_generation(path, generation, cancelled, || {
        publish(root, path, tag, &image.bytes, None)
    });
}

pub(super) fn accept_image(
    path: &Path,
    generation: u64,
    prior: Option<&Receipt>,
    image: &Image,
    source: Option<SourceRecord>,
    cancelled: &AtomicBool,
) -> io::Result<Option<Receipt>> {
    let root = cache_root(path).ok_or(io::ErrorKind::InvalidInput)?;
    let tag = format_tag(image).ok_or(io::ErrorKind::InvalidInput)?;
    if !(1..=RESPONSE_LIMIT).contains(&(image.bytes.len() as u64)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    with_current_generation(path, generation, cancelled, || {
        if !matches_prior(path, prior) {
            return Ok(None);
        }
        if let Some(source) = source {
            publish(root, path, tag, &image.bytes, Some(source)).map(Some)
        } else {
            remove_reference(path)?;
            Ok(Some(Receipt {
                content: content_id(tag, &image.bytes),
                source: None,
                checked_at: SystemTime::now(),
            }))
        }
    })?
    .transpose()
    .map(Option::flatten)
}

pub(super) fn accept_not_modified(
    path: &Path,
    generation: u64,
    prior: &Receipt,
    response: Validators,
    no_store: bool,
    cancelled: &AtomicBool,
) -> io::Result<Option<Receipt>> {
    let root = cache_root(path).ok_or(io::ErrorKind::InvalidInput)?;
    let Some(source) = &prior.source else {
        return Ok(None);
    };
    with_current_generation(path, generation, cancelled, || {
        if !matches_prior(path, Some(prior)) {
            return Ok(None);
        }
        if no_store {
            remove_reference(path)?;
            let mut receipt = prior.clone();
            receipt.source = None;
            return Ok(Some(receipt));
        }
        let blob = blob_path(root, &prior.content);
        OpenOptions::new()
            .write(true)
            .open(&blob)?
            .set_modified(SystemTime::now())?;
        let confirmed = SourceRecord {
            rendition: source.rendition,
            validators: source.validators.update_from(response),
        };
        write_reference(path, prior.content, Some(confirmed)).map(Some)
    })?
    .transpose()
    .map(Option::flatten)
}

fn matches_prior(path: &Path, prior: Option<&Receipt>) -> bool {
    read_entry_with_age(path, prior.is_some())
        .as_ref()
        .map(|entry| &entry.receipt)
        == prior
}

fn remove_reference(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn image_content(image: &Image) -> Option<ContentId> {
    Some(content_id(format_tag(image)?, &image.bytes))
}

fn format_tag(image: &Image) -> Option<u8> {
    match image.format {
        ImageFormat::Jpeg => Some(1),
        ImageFormat::Png => Some(2),
        ImageFormat::Webp => Some(3),
        _ => None,
    }
}

pub(super) fn with_current_generation<T>(
    path: &Path,
    generation: u64,
    cancelled: &AtomicBool,
    write: impl FnOnce() -> T,
) -> io::Result<Option<T>> {
    let root = cache_root(path).ok_or(io::ErrorKind::InvalidInput)?;
    let _lock = write_lock(root)?;
    Ok((!cancelled.load(Ordering::Acquire) && read_generation(root)? == generation).then(write))
}

fn cache_root(path: &Path) -> Option<&Path> {
    let provider = path.parent()?;
    let root = provider.parent()?;
    if root.file_name()? != OsStr::new(ARTWORK_DIRECTORY) {
        return None;
    }
    let name = provider.file_name()?;
    ProviderId::ALL
        .into_iter()
        .any(|provider| name == OsStr::new(provider.as_str()))
        .then_some(root)
}

fn read_tag(file: &mut File) -> Option<u8> {
    let mut tag = [0];
    file.read_exact(&mut tag).ok()?;
    Some(tag[0])
}

fn image_format(tag: u8) -> Option<ImageFormat> {
    match tag {
        1 => Some(ImageFormat::Jpeg),
        2 => Some(ImageFormat::Png),
        3 => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn read_payload(file: File, length: u64) -> Option<Vec<u8>> {
    if !(2..=RESPONSE_LIMIT + 1).contains(&length) {
        return None;
    }
    let mut bytes = Vec::with_capacity((length - 1) as usize);
    file.take(length).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 == length - 1).then_some(bytes)
}

fn read_blob(root: &Path, content: &ContentId) -> Option<Image> {
    let mut file = File::open(blob_path(root, content)).ok()?;
    let length = file.metadata().ok()?.len();
    let tag = read_tag(&mut file)?;
    let format = image_format(tag)?;
    let bytes = read_payload(file, length)?;
    (content_id(tag, &bytes) == *content).then(|| Image::from_bytes(format, bytes))
}

fn content_id(tag: u8, bytes: &[u8]) -> ContentId {
    let mut hash = Sha256::new();
    hash.update([tag]);
    hash.update(bytes);
    hash.finalize().into()
}

fn blob_path(root: &Path, content: &ContentId) -> PathBuf {
    let hash = sha2::digest::Output::<Sha256>::from(*content);
    root.join(BLOB_DIRECTORY).join(format!("{hash:x}.blob"))
}

fn write_lock(root: &Path) -> io::Result<File> {
    fs::create_dir_all(root)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("cache.lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn read_generation(root: &Path) -> io::Result<u64> {
    let file = match File::open(root.join(GENERATION_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut raw = [0; 8];
    if file.metadata()?.len() != raw.len() as u64 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    (&file).read_exact(&mut raw)?;
    Ok(u64::from_le_bytes(raw))
}

fn publish(
    root: &Path,
    path: &Path,
    tag: u8,
    bytes: &[u8],
    source: Option<SourceRecord>,
) -> io::Result<Receipt> {
    let content = content_id(tag, bytes);
    let blob = blob_path(root, &content);
    let now = SystemTime::now();
    let reused = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&blob)
        .is_ok_and(|mut file| {
            matches_payload(&mut file, tag, bytes).unwrap_or(false)
                && file.set_modified(now).is_ok()
        });
    if !reused {
        write_atomically(&blob, &[tag], bytes)?;
    }
    let receipt = write_reference(path, content, source)?;
    evict(root, path, &blob, MAX_BYTES);
    Ok(receipt)
}

fn write_reference(
    path: &Path,
    content: ContentId,
    source: Option<SourceRecord>,
) -> io::Result<Receipt> {
    if let Some(source) = &source {
        let mut body = Vec::from(content);
        body.extend(serde_json::to_vec(source).map_err(io::Error::other)?);
        write_atomically(path, &[VALIDATED_REFERENCE], &body)?;
    } else {
        write_atomically(path, &[CONTENT_REFERENCE], &content)?;
    }
    Ok(Receipt {
        content,
        source,
        checked_at: fs::metadata(path)?.modified()?,
    })
}

fn matches_payload(file: &mut File, tag: u8, bytes: &[u8]) -> io::Result<bool> {
    if file.metadata()?.len() != bytes.len() as u64 + 1 || read_tag(file) != Some(tag) {
        return Ok(false);
    }
    let mut buffer = [0u8; 8192];
    for chunk in bytes.chunks(buffer.len()) {
        let actual = &mut buffer[..chunk.len()];
        file.read_exact(actual)?;
        if actual != chunk {
            return Ok(false);
        }
    }
    Ok(true)
}

fn write_atomically(path: &Path, prefix: &[u8], bytes: &[u8]) -> io::Result<()> {
    let directory = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::Builder::new()
        .suffix(".pending")
        .tempfile_in(directory)?;
    temporary.write_all(prefix)?;
    temporary.write_all(bytes)?;
    temporary.persist(path).map_err(|failure| failure.error)?;
    Ok(())
}

pub(super) fn clear(profile_directory: &Path) -> io::Result<u64> {
    fs::create_dir_all(profile_directory)?;
    let root = fs::canonicalize(profile_directory)?.join(ARTWORK_DIRECTORY);
    checked_directory(&root)?;
    let _lock = write_lock(&root)?;
    let generation = read_generation(&root)?
        .checked_add(1)
        .ok_or(io::ErrorKind::InvalidData)?;
    write_atomically(&root.join(GENERATION_FILE), &[], &generation.to_le_bytes())?;
    let files = visit_managed_files(&root, |path| match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    });
    let palettes = Store::open(profile_directory)
        .and_then(|store| store.clear_artwork_palettes())
        .map_err(io::Error::other);
    files.and_then(|bytes| palettes.map(|()| bytes))
}

pub(super) fn usage(profile_directory: &Path) -> io::Result<u64> {
    let Some(root) = checked_root(profile_directory)? else {
        return Ok(0);
    };
    visit_managed_files(&root, |_| Ok(true))
}

fn checked_root(profile_directory: &Path) -> io::Result<Option<PathBuf>> {
    let profile = match fs::canonicalize(profile_directory) {
        Ok(profile) => profile,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let root = profile.join(ARTWORK_DIRECTORY);
    Ok(checked_directory(&root)?.then_some(root))
}

fn checked_directory(directory: &Path) -> io::Result<bool> {
    match fs::canonicalize(directory) {
        Ok(resolved) if resolved == directory => Ok(true),
        Ok(_) => Err(io::ErrorKind::InvalidInput.into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn visit_managed_files(
    root: &Path,
    mut visit: impl FnMut(&Path) -> io::Result<bool>,
) -> io::Result<u64> {
    let folders = ProviderId::ALL
        .into_iter()
        .map(|provider| (root.join(provider.as_str()), "cover"))
        .chain([(root.join(BLOB_DIRECTORY), "blob"), (root.to_owned(), "")]);
    let mut bytes = 0;
    for (folder, extension) in folders {
        if !checked_directory(&folder)? {
            continue;
        }
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let path = entry.path();
            if !managed_name(&path, extension) || !entry.file_type()?.is_file() {
                continue;
            }
            let length = match entry.metadata() {
                Ok(metadata) => metadata.len(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if visit(&path)? {
                bytes += length;
            }
        }
    }
    Ok(bytes)
}

fn managed_name(path: &Path, extension: &str) -> bool {
    let (Some(suffix), Some(stem)) = (path.extension(), path.file_stem().and_then(OsStr::to_str))
    else {
        return false;
    };
    (suffix == extension && stem.len() == 64 && stem.bytes().all(|byte| byte.is_ascii_hexdigit()))
        || (suffix == TEMPORARY_EXTENSION && stem.starts_with(".tmp"))
}

struct Entry {
    path: PathBuf,
    modified: SystemTime,
    bytes: u64,
}

fn prune_directory(
    directory: &Path,
    extension: &str,
    limit: usize,
    published: &Path,
) -> Vec<Entry> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut live: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let suffix = path.extension()?;
            if !managed_name(&path, extension) || !entry.file_type().ok()?.is_file() {
                return None;
            }
            // Atomic writes leave these files only if a process stopped before publish.
            if suffix == TEMPORARY_EXTENSION {
                let _ = fs::remove_file(path);
                return None;
            }
            let metadata = entry.metadata().ok()?;
            let modified = metadata.modified().ok()?;
            if modified.elapsed().is_ok_and(|age| age >= MAX_AGE) && fs::remove_file(&path).is_ok()
            {
                return None;
            }
            Some(Entry {
                path,
                modified,
                bytes: metadata.len(),
            })
        })
        .collect();
    let mut excess = live.len().saturating_sub(limit);
    if excess > 0 {
        live.sort_unstable_by_key(|entry| (entry.path == published, entry.modified));
        live.retain(|entry| {
            if excess > 0 && fs::remove_file(&entry.path).is_ok() {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
    live
}

fn evict(root: &Path, published: &Path, blob: &Path, budget: u64) {
    let mut live = prune_directory(&root.join(BLOB_DIRECTORY), "blob", MAX_BLOBS, blob);
    for provider in ProviderId::ALL {
        live.extend(prune_directory(
            &root.join(provider.as_str()),
            "cover",
            MAX_FILES,
            published,
        ));
    }
    let mut bytes: u64 = live.iter().map(|entry| entry.bytes).sum();
    if bytes <= budget {
        return;
    }
    live.sort_unstable_by_key(|entry| {
        (
            entry.path == blob || entry.path == published,
            entry.modified,
        )
    });
    for entry in live {
        if bytes <= budget {
            break;
        }
        if fs::remove_file(entry.path).is_ok() {
            bytes -= entry.bytes;
        }
    }
}

#[cfg(test)]
mod tests;
