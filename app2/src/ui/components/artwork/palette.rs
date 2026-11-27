use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use image::DynamicImage;

use crate::storage::Store;

use super::{ArtworkColors, colors, decode, disk};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Palette {
    pub(super) colors: Option<ArtworkColors>,
    pub(super) tint: u32,
}

impl Palette {
    fn encode(self) -> Vec<u8> {
        let mut data = self.tint.to_le_bytes().to_vec();
        data.extend(colors::encode(self.colors));
        data
    }

    fn decode(data: &[u8]) -> Option<Self> {
        let (tint, colors) = data.split_first_chunk::<4>()?;
        let tint = u32::from_le_bytes(*tint);
        if tint > 0x00ff_ffff {
            return None;
        }
        Some(Self {
            tint,
            colors: colors::decode(colors)?,
        })
    }
}

pub(super) fn resolve(
    store: Option<&Store>,
    entry: Option<(&Path, u64)>,
    content: &disk::ContentId,
    image: &DynamicImage,
    cancelled: &AtomicBool,
) -> Option<Palette> {
    let prepare = || {
        let sample = decode::palette_sample(image);
        move || {
            #[cfg(all(test, feature = "capture"))]
            EXTRACTIONS.fetch_add(1, Ordering::Relaxed);
            Palette {
                colors: colors::extract(&sample),
                tint: decode::tint(image),
            }
        }
    };
    let fallback = || (!cancelled.load(Ordering::Acquire)).then(|| prepare()());
    let Some((path, generation)) = entry else {
        return fallback();
    };
    let Some(store) = store else {
        return fallback();
    };
    resolve_with(
        store,
        path,
        content,
        colors::ALGORITHM_VERSION,
        generation,
        cancelled,
        prepare,
    )
}

fn resolve_with<F: FnOnce() -> Palette>(
    store: &Store,
    path: &Path,
    content: &disk::ContentId,
    version: i64,
    generation: u64,
    cancelled: &AtomicBool,
    prepare: impl FnOnce() -> F,
) -> Option<Palette> {
    let read = || {
        store
            .artwork_palette(content, version, generation)
            .ok()
            .flatten()
            .and_then(|bytes| Palette::decode(&bytes))
    };
    if let Some(palette) = read() {
        return Some(palette);
    }
    // Decoding and resizing precede the file lock. Only 64-pixel extraction and
    // publication are serialized, so another requester can reuse these colors.
    let mut analyse = Some(prepare());
    let saved = disk::with_current_generation(path, generation, cancelled, || {
        if let Some(palette) = read() {
            return palette;
        }
        let palette = analyse.take().expect("unused artwork sample")();
        if !cancelled.load(Ordering::Acquire) {
            let _ = store.save_artwork_palette(
                content,
                version,
                generation,
                &palette.encode(),
                disk::MAX_BLOBS as i64,
            );
        }
        palette
    });
    saved.unwrap_or_else(|_| {
        (!cancelled.load(Ordering::Acquire)).then(|| analyse.expect("unused artwork sample")())
    })
}

#[cfg(all(test, feature = "capture"))]
pub(super) static EXTRACTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use std::{
        sync::{Barrier, atomic::AtomicUsize},
        thread,
    };

    use super::*;

    #[test]
    fn palette_uses_one_extraction_per_hash_version_and_clear_generation() {
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(profile.path()).unwrap();
        let original = disk::path(profile.path(), "https://i.scdn.co/image/palette", None).unwrap();
        let alias = disk::path(
            profile.path(),
            "https://i.ytimg.com/vi/palette/hqdefault.jpg",
            None,
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let count = AtomicUsize::new(0);
        let active_version = colors::ALGORITHM_VERSION;
        let profile_generation = disk::generation(&original).unwrap();
        let content = [7; 32];
        let ready = Barrier::new(2);
        let expected = Palette {
            colors: None,
            tint: 0x833351,
        };
        let prepare = || {
            || {
                count.fetch_add(1, Ordering::Relaxed);
                expected
            }
        };
        thread::scope(|scope| {
            for path in [&original, &alias] {
                let (ready, store, cancel) = (&ready, &store, &cancel);
                scope.spawn(move || {
                    ready.wait();
                    assert_eq!(
                        resolve_with(
                            store,
                            path,
                            &content,
                            active_version,
                            profile_generation,
                            cancel,
                            prepare
                        ),
                        Some(expected)
                    );
                });
            }
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
        let reopened = Store::open(profile.path()).unwrap();
        assert_eq!(
            resolve_with(
                &reopened,
                &original,
                &content,
                active_version,
                profile_generation,
                &cancel,
                prepare
            ),
            Some(expected)
        );
        assert_eq!(count.load(Ordering::Relaxed), 1);

        let other_content = [8; 32];
        assert_eq!(
            resolve_with(
                &reopened,
                &original,
                &other_content,
                active_version,
                profile_generation,
                &cancel,
                prepare
            ),
            Some(expected)
        );
        assert_eq!(count.load(Ordering::Relaxed), 2);
        assert_eq!(
            resolve_with(
                &reopened,
                &original,
                &content,
                active_version + 1,
                profile_generation,
                &cancel,
                prepare
            ),
            Some(expected)
        );
        assert_eq!(count.load(Ordering::Relaxed), 3);
        assert!(
            reopened
                .artwork_palette(&content, active_version, profile_generation)
                .unwrap()
                .is_some()
        );

        disk::clear(profile.path()).unwrap();
        let after_clear = disk::generation(&original).unwrap();
        assert_ne!(profile_generation, after_clear);
        assert!(
            resolve_with(
                &reopened,
                &original,
                &content,
                active_version + 1,
                profile_generation,
                &cancel,
                prepare
            )
            .is_none()
        );
        assert!(
            reopened
                .artwork_palette(&content, active_version + 1, profile_generation)
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .artwork_palette(&content, active_version + 1, after_clear)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            resolve_with(
                &reopened,
                &original,
                &content,
                active_version + 1,
                after_clear,
                &cancel,
                prepare
            ),
            Some(expected)
        );
        assert_eq!(count.load(Ordering::Relaxed), 4);
    }
}
