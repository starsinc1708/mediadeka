use std::{
    fs::{self, File},
    io::Read as _,
    path::Path,
    time::SystemTime,
};

use lofty::{config::ParseOptions, file::TaggedFileExt as _, probe::Probe, tag::ItemKey};

use super::{Lyrics, LyricsVersions, MAX_TEXT_BYTES, Source, Timing, parse_lrc, ttml};

const SIDECARS: [&str; 3] = ["ttml", "lrc", "txt"];
pub(super) type Revision = [Option<(SystemTime, u64)>; 4];

pub(super) fn revision(audio: &Path) -> Option<Revision> {
    let mut revision = Revision::default();
    let sidecars = SIDECARS.map(|extension| audio.with_extension(extension));
    for (slot, path) in revision
        .iter_mut()
        .zip(std::iter::once(audio).chain(sidecars.iter().map(|path| path.as_path())))
    {
        *slot = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Some((metadata.modified().ok()?, metadata.len())),
            // A linked audio file may change without changing the link's metadata.
            Ok(metadata) if metadata.is_symlink() => return None,
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return None,
        };
    }
    Some(revision)
}

pub(super) fn read(audio: &Path) -> Option<LyricsVersions> {
    let versions = SIDECARS.into_iter().filter_map(|extension| {
        let text = sidecar(&audio.with_extension(extension))?;
        if extension == "ttml" {
            ttml::parse(&text)
        } else {
            decode(&text)
        }
    });
    select_versions(versions).or_else(|| embedded(audio))
}

fn embedded(audio: &Path) -> Option<LyricsVersions> {
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false);
    let file = Probe::open(audio)
        .ok()?
        .guess_file_type()
        .ok()?
        .options(options)
        .read()
        .ok()?;
    let versions = file
        .tags()
        .iter()
        .flat_map(|tag| {
            [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
                .into_iter()
                .flat_map(|key| tag.get_items(key))
        })
        .filter_map(|item| decode(item.value().text()?));
    select_versions(versions)
}

fn select_versions(versions: impl Iterator<Item = Lyrics>) -> Option<LyricsVersions> {
    let mut synchronized: Option<Lyrics> = None;
    let mut plain = None;
    for lyrics in versions.filter(Lyrics::usable) {
        let timing = lyrics.timing();
        if timing == Timing::Plain {
            plain.get_or_insert(lyrics);
        } else if synchronized
            .as_ref()
            .is_none_or(|best| timing > best.timing())
        {
            synchronized = Some(lyrics);
        }
    }
    let preferred = synchronized.or_else(|| plain.take())?;
    Some(LyricsVersions::from_source(preferred, plain))
}

fn sidecar(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_TEXT_BYTES as u64 {
        return None;
    }
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(MAX_TEXT_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= MAX_TEXT_BYTES).then_some(text)
}

fn decode(text: &str) -> Option<Lyrics> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        return None;
    }
    if text.starts_with("<tt") || text.starts_with("<?xml") {
        return ttml::parse(text);
    }
    parse_lrc(Source::Local, text).or_else(|| Some(Lyrics::plain(Source::Local, text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_revision_follows_sidecar_edits_and_creation() {
        let profile = tempfile::tempdir().unwrap();
        let audio = profile.path().join("song.wav");
        File::create(&audio).unwrap();
        let original = revision(&audio).unwrap();
        let sidecar = audio.with_extension("lrc");
        fs::write(&sidecar, "[00:01.00]First").unwrap();
        let added = revision(&audio).unwrap();
        assert_ne!(original, added);
        fs::write(&sidecar, "[00:03.00]Changed cue").unwrap();
        assert_ne!(added, revision(&audio).unwrap());
        fs::remove_file(&sidecar).unwrap();
        assert_eq!(revision(&audio), Some(original));
    }
}
