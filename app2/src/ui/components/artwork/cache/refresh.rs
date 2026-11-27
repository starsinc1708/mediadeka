use std::{
    sync::{Arc, OnceLock, PoisonError, atomic::Ordering},
    time::{Duration, Instant},
};

use gpui::App;

use super::{
    Artwork, ArtworkCache, ArtworkImage, ArtworkRequest, ArtworkSource, ORIGINAL_ENTRIES,
    OriginalArtwork, source,
};

const RETRY_ON_USE: Duration = Duration::from_secs(60 * 60);

pub(super) struct Watch {
    revalidation: Option<source::Revalidation>,
    next: Instant,
}

impl Watch {
    pub(super) fn new(revalidation: source::Revalidation) -> Self {
        let next = Instant::now() + revalidation.receipt.fresh_for();
        Self {
            revalidation: Some(revalidation),
            next,
        }
    }

    fn due(&self) -> Option<&source::Revalidation> {
        (Instant::now() >= self.next)
            .then_some(self.revalidation.as_ref())
            .flatten()
    }
}

enum Finished {
    Same(Option<source::Revalidation>),
    Changed(OriginalArtwork),
    Failed,
}

#[cfg(all(test, feature = "capture"))]
pub(super) fn request_for_capture(
    image: &ArtworkImage,
    request: &ArtworkRequest,
    cx: &mut App,
) -> bool {
    let Some(watch) = &image.watch else {
        return false;
    };
    if cx.default_global::<ArtworkCache>().refreshing.is_some() {
        return false;
    }
    watch.lock().unwrap_or_else(PoisonError::into_inner).next = Instant::now();
    start(request, image, cx);
    cx.default_global::<ArtworkCache>().refreshing.is_some()
}

pub(super) fn start(request: &ArtworkRequest, image: &ArtworkImage, cx: &mut App) {
    let cache = cx.default_global::<ArtworkCache>();
    if cache.refreshing.is_some() {
        return;
    }
    let Some(watch) = image.watch.as_ref() else {
        return;
    };
    let Some(check) = watch
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .due()
        .cloned()
    else {
        return;
    };
    let watch = Arc::clone(watch);
    let previous = check.receipt.content;
    let cancelled = Arc::clone(&cache.cancelled);
    let palette_store = Arc::clone(&cache.palette_store);
    let artwork_source = request.source.clone();
    let worker_source = artwork_source.clone();
    let task = cx.spawn(async move |cx| {
        let stop = Arc::clone(&cancelled);
        let result = cx
            .background_executor()
            .spawn(async move {
                match source::refresh(&worker_source, &check, &stop) {
                    source::Refresh::Same(checked) => Finished::Same(checked),
                    source::Refresh::Changed(image) => {
                        OriginalArtwork::prepare(image, palette_store.get(), &stop)
                            .map_or(Finished::Failed, |(image, _)| Finished::Changed(image))
                    }
                    source::Refresh::Failed => Finished::Failed,
                }
            })
            .await;
        cx.update(|cx| {
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            cx.default_global::<ArtworkCache>().refreshing = None;
            match result {
                Finished::Same(checked) => {
                    let mut state = watch.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(checked) = checked {
                        *state = Watch::new(checked);
                    } else {
                        state.revalidation = None;
                    }
                }
                Finished::Changed(image) => install(&artwork_source, previous, image, cx),
                Finished::Failed => {
                    watch.lock().unwrap_or_else(PoisonError::into_inner).next =
                        Instant::now() + RETRY_ON_USE
                }
            }
            cx.refresh_windows();
        });
    });
    cx.default_global::<ArtworkCache>().refreshing = Some(task);
}

fn install(
    source: &ArtworkSource,
    previous: super::disk::ContentId,
    image: OriginalArtwork,
    cx: &mut App,
) {
    let cache = cx.default_global::<ArtworkCache>();
    let existing = cache.originals.iter().position(|(key, _)| key == source);
    let incoming = image.content;
    let mut install_original = true;
    if let Some(index) = existing {
        let current = &cache.originals[index].1;
        if let Some(newest) = current.get().and_then(Option::as_ref) {
            if newest.content == incoming {
                install_original = false;
            } else if newest.content != previous {
                return;
            }
        }
        if install_original {
            cache.originals.remove(index);
        }
    }
    if install_original {
        cache
            .originals
            .push_back((source.clone(), Arc::new(OnceLock::from(Some(image)))));
        if cache.originals.len() > ORIGINAL_ENTRIES {
            cache.originals.pop_front();
        }
    }
    let requests: Vec<_> = cache
        .recent
        .iter_mut()
        .filter(|entry| &entry.request.source == source)
        .filter_map(|variant| {
            if variant
                .ready
                .as_ref()
                .is_some_and(|image| image.content != previous)
            {
                None
            } else {
                Some((variant.request.clone(), variant.ready.take()))
            }
        })
        .collect();
    for (request, image) in requests {
        if let Some(image) = image {
            if let Some(previous) = cx.default_global::<ArtworkCache>().take_stale(&request) {
                cx.defer(move |cx| cx.drop_image(previous.image, None));
            }
            cx.default_global::<ArtworkCache>()
                .stale_variants
                .push((request.clone(), image));
        }
        cx.remove_asset::<Artwork>(&request);
    }
}
