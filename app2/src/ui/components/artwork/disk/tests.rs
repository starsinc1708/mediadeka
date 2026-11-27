use std::{sync::Barrier, thread};

use super::*;
use crate::storage::Store;

const SPOTIFY: &str = "https://i.scdn.co/image/cover";
const YOUTUBE: &str = "https://i.ytimg.com/vi/example/hqdefault.jpg";

fn cover(profile: &Path, url: &str) -> PathBuf {
    path(profile, url, None).unwrap()
}

#[test]
fn cover_cache_reopens_legacy_entries_and_repairs_a_corrupt_shared_blob() {
    let temporary = tempfile::tempdir().unwrap();
    let path = cover(temporary.path(), SPOTIFY);
    let alias = cover(temporary.path(), YOUTUBE);
    let root = cache_root(&path).unwrap();
    let image = Image::from_bytes(ImageFormat::Jpeg, vec![1, 3, 5]);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, [1, 1, 3, 5]).unwrap();
    assert_eq!(read(&path), Some(image.clone()));
    let original_fixture = temporary.path().join("fixture.cover");
    fs::copy(&path, &original_fixture).unwrap();
    assert_eq!(read(&original_fixture), Some(image.clone()));

    save(&path, &image);
    save(&alias, &image);
    assert_eq!(read(&path), Some(image.clone()));
    assert_eq!(read(&alias), Some(image.clone()));
    assert_eq!(fs::read(&path).unwrap(), fs::read(&alias).unwrap());
    assert_eq!(fs::read_dir(root.join(BLOB_DIRECTORY)).unwrap().count(), 1);

    let blob = blob_path(root, &content_id(1, &image.bytes));
    fs::write(blob, [1, 1, 3, 9]).unwrap();
    assert!(read(&path).is_none());
    assert!(read(&alias).is_none());
    save(&alias, &image);
    assert_eq!(read(&path), Some(image.clone()));
    fs::write(&path, [CONTENT_REFERENCE, 1, 3, 5]).unwrap();
    assert!(read(&path).is_none());
    assert_eq!(read(&alias), Some(image));
}

#[test]
fn concurrent_artwork_writes_share_one_blob_without_sharing_across_profiles() {
    let temporary = tempfile::tempdir().unwrap();
    let first = temporary.path().join("first");
    let second = temporary.path().join("second");
    let writers = 8;
    let ready = Barrier::new(writers);
    let image = Image::from_bytes(ImageFormat::Png, vec![1, 3, 5]);
    let paths: Vec<_> = (0..writers)
        .map(|index| cover(&first, &format!("{SPOTIFY}/{index}")))
        .collect();
    thread::scope(|scope| {
        for path in &paths {
            let ready = &ready;
            let image = &image;
            scope.spawn(move || {
                ready.wait();
                save(path, image);
            });
        }
    });
    let root = cache_root(&paths[0]).unwrap();
    assert_eq!(fs::read_dir(root.join(BLOB_DIRECTORY)).unwrap().count(), 1);
    for path in paths {
        assert_eq!(read(&path), Some(image.clone()));
    }
    assert!(read(&cover(&second, SPOTIFY)).is_none());
    let other = cover(&second, SPOTIFY);
    save(&other, &image);
    let content = content_id(2, &image.bytes);
    fs::remove_file(blob_path(cache_root(&other).unwrap(), &content)).unwrap();
    assert!(read(&other).is_none());
    assert!(blob_path(&first.join(ARTWORK_DIRECTORY), &content).exists());
}

#[test]
fn artwork_disk_budget_counts_unique_payloads_and_legacy_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let path = cover(temporary.path(), SPOTIFY);
    let alias = cover(temporary.path(), YOUTUBE);
    let root = cache_root(&path).unwrap();
    let image = Image::from_bytes(ImageFormat::Jpeg, vec![1; 128]);
    save(&path, &image);
    save(&alias, &image);
    let legacy = cover(temporary.path(), &format!("{SPOTIFY}/old"));
    fs::write(&legacy, [1; 160]).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&legacy)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(60))
        .unwrap();
    assert!(read(&legacy).is_some());

    let bytes = image.bytes.len() as u64 + 1 + 2 * REFERENCE_LENGTH;
    let blob = blob_path(root, &content_id(1, &image.bytes));
    evict(root, &alias, &blob, bytes);
    assert!(!legacy.exists());
    assert_eq!(read(&path), Some(image.clone()));
    assert_eq!(read(&alias), Some(image));
}

#[test]
fn artwork_eviction_expires_aliases_and_unpublished_files() {
    let temporary = tempfile::tempdir().unwrap();
    let path = cover(temporary.path(), SPOTIFY);
    let alias = cover(temporary.path(), YOUTUBE);
    let image = Image::from_bytes(ImageFormat::Jpeg, vec![1, 3, 5]);
    save(&path, &image);
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - MAX_AGE)
        .unwrap();
    let unfinished = path.parent().unwrap().join(".tmp-unfinished.pending");
    fs::write(&unfinished, [1; 128]).unwrap();
    assert!(read(&path).is_none());
    save(&alias, &image);
    assert!(!path.exists());
    assert!(!unfinished.exists());
    assert_eq!(read(&alias), Some(image));
}

#[test]
fn selective_clear_preserves_profile_data_and_rejects_pre_clear_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let profile = temporary.path();
    let store = Store::open(profile).unwrap();
    let playlist = store
        .create_local_playlist("Сохранённый плейлист", &[])
        .unwrap();
    let other = profile.join("downloaded.flac");
    fs::write(&other, b"preserved").unwrap();
    let first = cover(profile, SPOTIFY);
    let second = cover(profile, YOUTUBE);
    let generation_before_clear = generation(&first).unwrap();
    let image = Image::from_bytes(ImageFormat::Png, vec![1, 3, 5]);
    save(&first, &image);
    save(&second, &image);
    let legacy = cover(profile, &format!("{SPOTIFY}/legacy"));
    fs::write(&legacy, [1, 1, 3, 5]).unwrap();
    let notes = first.parent().unwrap().join("notes.cover");
    fs::write(&notes, b"preserved").unwrap();
    let partial = first.parent().unwrap().join(".tmp-test.pending");
    fs::write(&partial, [9]).unwrap();
    let used = 2 * REFERENCE_LENGTH + 4 + 4 + 1;
    assert_eq!(usage(profile).unwrap(), used);

    assert_eq!(clear(profile).unwrap(), used);
    assert_eq!(usage(profile).unwrap(), 0);
    assert_eq!(fs::read(&notes).unwrap(), b"preserved");
    assert_eq!(fs::read(&other).unwrap(), b"preserved");
    assert_eq!(
        store.local_playlist(playlist).unwrap().unwrap().name,
        "Сохранённый плейлист"
    );

    save_current(
        &first,
        &image,
        generation_before_clear,
        &AtomicBool::new(false),
    );
    assert!(read(&first).is_none());
    let after_clear = generation(&first).unwrap();
    assert_ne!(after_clear, generation_before_clear);
    save_current(&first, &image, after_clear, &AtomicBool::new(true));
    assert!(read(&first).is_none());
    save_current(&first, &image, after_clear, &AtomicBool::new(false));
    assert_eq!(read(&first), Some(image));
}

#[test]
fn clearing_an_empty_cache_invalidates_the_first_pending_write() {
    let temporary = tempfile::tempdir().unwrap();
    let profile = temporary.path();
    let path = cover(profile, SPOTIFY);
    let generation_before_clear = generation(&path).unwrap();
    assert_eq!(clear(profile).unwrap(), 0);
    let image = Image::from_bytes(ImageFormat::Png, vec![1, 3, 5]);
    save_current(
        &path,
        &image,
        generation_before_clear,
        &AtomicBool::new(false),
    );
    assert!(read(&path).is_none());
}

#[test]
fn cache_paths_preserve_spotify_files_and_keep_card_renditions_separate() {
    let profile = Path::new("profile");
    assert_eq!(
        path(profile, SPOTIFY, None).unwrap(),
        profile
            .join("artwork/spotify")
            .join(format!("{:x}.cover", Sha256::digest(SPOTIFY)))
    );
    for (provider, url) in [
        ("soundcloud", "https://i1.sndcdn.com/artworks-abc-large.jpg"),
        (
            "yandex-music",
            "https://avatars.yandex.net/get-music-content/cover",
        ),
        ("youtube-music", YOUTUBE),
    ] {
        assert_eq!(
            path(profile, url, None).unwrap().parent().unwrap(),
            profile.join("artwork").join(provider)
        );
    }
    let thumbnail = "https://i1.sndcdn.com/artworks-abc-large.jpg";
    let card = "https://i1.sndcdn.com/artworks-abc-t500x500.jpg";
    assert_ne!(
        path(profile, thumbnail, None),
        path(profile, thumbnail, Some(card))
    );
    assert!(path(profile, thumbnail, Some(SPOTIFY)).is_none());
    assert!(path(profile, "http://127.0.0.1/artwork", None).is_none());
}
