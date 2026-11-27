use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{App, Asset, Background, Global, Hsla, Image, RenderImage, Task, rgb};

use crate::storage::Store;

use super::{ArtworkColors, ArtworkEdge, ArtworkSource, decode, disk, glow, palette, source};

mod refresh;
mod variants;

pub(super) const CACHE_ENTRIES: usize = 64;
const DECODED_BYTE_BUDGET: usize = 24 * 1024 * 1024;
const ORIGINAL_ENTRIES: usize = 24;

#[derive(Clone, Eq, Hash, PartialEq)]
pub(super) struct ArtworkRequest {
    pub(super) source: ArtworkSource,
    pub(super) edge: ArtworkEdge,
    pub(super) treatment: decode::Treatment,
}

pub(super) enum Artwork {}

#[derive(Clone)]
pub(super) struct ArtworkImage {
    pub(super) image: Arc<RenderImage>,
    pub(super) tint: Hsla,
    pub(super) glow: Background,
    pub(super) bytes: usize,
    pub(super) dimensions: [u32; 2],
    pub(super) colors: Option<ArtworkColors>,
    pub(super) content: disk::ContentId,
    watch: Option<Arc<Mutex<refresh::Watch>>>,
}

impl ArtworkImage {
    fn new(image: Arc<RenderImage>, original: &OriginalArtwork) -> Self {
        let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
        Self {
            image,
            tint: original.tint,
            glow: original.glow,
            bytes,
            dimensions: original.dimensions,
            colors: original.colors,
            content: original.content,
            watch: original.watch.clone(),
        }
    }
}

struct OriginalArtwork {
    image: Image,
    tint: Hsla,
    glow: Background,
    dimensions: [u32; 2],
    colors: Option<ArtworkColors>,
    content: disk::ContentId,
    watch: Option<Arc<Mutex<refresh::Watch>>>,
}

impl OriginalArtwork {
    fn prepare(
        source::Ready {
            image,
            decoded,
            content,
            revalidation,
        }: source::Ready,
        store: Option<&Store>,
        cancelled: &AtomicBool,
    ) -> Option<(Self, image::DynamicImage)> {
        let dimensions = [decoded.width(), decoded.height()];
        let location = revalidation
            .as_ref()
            .map(|check| (check.path.as_path(), check.generation));
        let palette = palette::resolve(store, location, &content, &decoded, cancelled)?;
        let tint = rgb(palette.tint).into();
        let watch = revalidation.map(|check| Arc::new(Mutex::new(refresh::Watch::new(check))));
        Some((
            Self {
                image,
                tint,
                glow: glow(tint),
                dimensions,
                colors: palette.colors,
                content,
                watch,
            },
            decoded,
        ))
    }
}

type OriginalSlot = Arc<OnceLock<Option<OriginalArtwork>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) enum ArtworkClearStatus {
    #[default]
    Idle,
    Clearing,
    Cleared,
    Failed,
}

pub(super) struct ArtworkCache {
    recent: VecDeque<RecentVariant>,
    decoded_bytes: usize,
    originals: VecDeque<(ArtworkSource, OriginalSlot)>,
    profile_directory: Option<PathBuf>,
    palette_store: Arc<OnceLock<Store>>,
    cancelled: Arc<AtomicBool>,
    clear_status: ArtworkClearStatus,
    clear_task: Option<Task<()>>,
    refreshing: Option<Task<()>>,
    stale_variants: Vec<(ArtworkRequest, ArtworkImage)>,
}

pub(super) struct RecentVariant {
    request: ArtworkRequest,
    bytes: usize,
    ready: Option<ArtworkImage>,
}

impl Global for ArtworkCache {}

impl Default for ArtworkCache {
    fn default() -> Self {
        Self::new(Store::profile_directory().ok())
    }
}

impl ArtworkCache {
    pub(super) fn new(profile_directory: Option<PathBuf>) -> Self {
        Self {
            recent: VecDeque::new(),
            decoded_bytes: 0,
            originals: VecDeque::new(),
            profile_directory,
            palette_store: Arc::new(OnceLock::new()),
            cancelled: Arc::new(AtomicBool::new(false)),
            clear_status: ArtworkClearStatus::Idle,
            clear_task: None,
            refreshing: None,
            stale_variants: Vec::new(),
        }
    }

    pub(super) fn touch(
        &mut self,
        request: ArtworkRequest,
        bytes: usize,
        ready: Option<ArtworkImage>,
    ) -> Vec<RecentVariant> {
        if let Some(index) = self
            .recent
            .iter()
            .position(|entry| entry.request == request)
            && let Some(previous) = self.recent.remove(index)
        {
            self.decoded_bytes -= previous.bytes;
        }
        self.recent.push_back(RecentVariant {
            request,
            bytes,
            ready,
        });
        self.decoded_bytes += bytes;
        let mut evicted = Vec::new();
        while self.decoded_bytes > DECODED_BYTE_BUDGET || self.recent.len() > CACHE_ENTRIES {
            let Some(variant) = self.recent.pop_front() else {
                break;
            };
            self.decoded_bytes -= variant.bytes;
            evicted.push(variant);
        }
        evicted
    }

    #[cfg(feature = "capture")]
    pub(super) fn request_for_capture(&self, source: &ArtworkSource) -> Option<ArtworkRequest> {
        self.recent
            .iter()
            .rev()
            .find_map(|entry| (&entry.request.source == source).then(|| entry.request.clone()))
    }

    fn original(&mut self, source: &ArtworkSource) -> OriginalSlot {
        let slot = self
            .originals
            .iter()
            .position(|(entry, _)| entry == source)
            .and_then(|index| self.originals.remove(index))
            .map_or_else(|| Arc::new(OnceLock::new()), |(_, slot)| slot);
        self.originals
            .push_back((source.clone(), Arc::clone(&slot)));
        if self.originals.len() > ORIGINAL_ENTRIES {
            self.originals.pop_front();
        }
        slot
    }

    fn take_stale(&mut self, request: &ArtworkRequest) -> Option<ArtworkImage> {
        let index = self
            .stale_variants
            .iter()
            .position(|(old, _)| old == request)?;
        Some(self.stale_variants.swap_remove(index).1)
    }
}

pub(super) fn cover_revision(url: &str, cx: &App) -> Option<disk::ContentId> {
    let cache = cx.try_global::<ArtworkCache>()?;
    let (_, slot) = cache
        .originals
        .iter()
        .find(|(source, _)| source.larger.is_none() && source.url.as_ref() == url)?;
    Some(slot.get()?.as_ref()?.content)
}

pub(in crate::ui) fn artwork_clear_status(cx: &App) -> ArtworkClearStatus {
    cx.try_global::<ArtworkCache>()
        .map_or(ArtworkClearStatus::Idle, |cache| cache.clear_status)
}

pub(in crate::ui) fn artwork_disk_usage(
    profile_directory: &std::path::Path,
) -> std::io::Result<u64> {
    disk::usage(profile_directory)
}

pub(in crate::ui) fn clear_artwork_cache(cx: &mut App) {
    let cache = cx.default_global::<ArtworkCache>();
    if cache.clear_task.is_some() {
        return;
    }
    let Some(profile) = cache.profile_directory.clone() else {
        cache.clear_status = ArtworkClearStatus::Failed;
        cx.refresh_windows();
        return;
    };
    cache.clear_status = ArtworkClearStatus::Clearing;
    cache.cancelled.store(true, Ordering::Release);
    cache.refreshing = None;
    cache.originals.clear();
    cache.decoded_bytes = 0;
    for variant in std::mem::take(&mut cache.recent) {
        discard_variant(variant, cx);
    }

    let task = cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { disk::clear(&profile) })
            .await;
        cx.update(|cx| {
            let cache = cx.default_global::<ArtworkCache>();
            cache.cancelled = Arc::new(AtomicBool::new(false));
            cache.clear_status = if result.is_ok() {
                ArtworkClearStatus::Cleared
            } else {
                ArtworkClearStatus::Failed
            };
            cache.clear_task = None;
            cx.refresh_windows();
        });
    });
    cx.default_global::<ArtworkCache>().clear_task = Some(task);
    cx.refresh_windows();
}

pub(super) fn discard_variant(variant: RecentVariant, cx: &mut App) {
    let previous = cx
        .default_global::<ArtworkCache>()
        .take_stale(&variant.request);
    for artwork in [previous, variant.ready].into_iter().flatten() {
        // A canvas in the current frame can still paint this image.
        cx.defer(move |cx| cx.drop_image(artwork.image, None));
    }
    cx.remove_asset::<Artwork>(&variant.request);
}

#[cfg(all(test, feature = "capture"))]
pub(super) fn request_revalidation_for_capture(request: &ArtworkRequest, cx: &mut App) -> bool {
    let Some(image) = cx.fetch_asset::<Artwork>(request).flatten() else {
        return false;
    };
    refresh::request_for_capture(&image, request, cx)
}

pub(super) fn present_result(
    request: &ArtworkRequest,
    value: Option<Option<ArtworkImage>>,
    cx: &mut App,
) -> (Option<Option<ArtworkImage>>, usize) {
    if let Some(current) = &value {
        if let Some(previous) = cx.default_global::<ArtworkCache>().take_stale(request) {
            cx.defer(move |cx| cx.drop_image(previous.image, None));
        }
        if let Some(image) = current {
            refresh::start(request, image, cx);
        }
        let bytes = current.as_ref().map_or(0, |artwork| artwork.bytes);
        return (value, bytes);
    }
    let stale = cx
        .default_global::<ArtworkCache>()
        .stale_variants
        .iter()
        .find(|(old, _)| old == request)
        .map(|(_, image)| image.clone());
    let bytes = stale.as_ref().map_or(0, |artwork| artwork.bytes) + request.edge.byte_bound();
    (stale.map(Some), bytes)
}

impl Asset for Artwork {
    type Source = ArtworkRequest;
    type Output = Option<ArtworkImage>;

    fn load(
        request: ArtworkRequest,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let cache = cx.default_global::<ArtworkCache>();
        let profile_directory = cache.profile_directory.clone();
        let palette_store = Arc::clone(&cache.palette_store);
        let slot = cache.original(&request.source);
        let cancelled = Arc::clone(&cache.cancelled);
        async move {
            if cancelled.load(Ordering::Acquire) {
                return None;
            }
            let mut first_variant = None;
            let original = slot
                .get_or_init(|| {
                    let ready =
                        source::load(&request.source, profile_directory.as_deref(), &cancelled)?;
                    if palette_store.get().is_none()
                        && let Some(store) = profile_directory
                            .as_deref()
                            .and_then(|profile| Store::open(profile).ok())
                    {
                        let _ = palette_store.set(store);
                    }
                    let (original, decoded) =
                        OriginalArtwork::prepare(ready, palette_store.get(), &cancelled)?;
                    first_variant = Some(ArtworkImage::new(
                        request.edge.render(decoded, request.treatment),
                        &original,
                    ));
                    Some(original)
                })
                .as_ref()?;
            let result = first_variant.or_else(|| {
                let decoded = decode::decode(&original.image)?;
                Some(ArtworkImage::new(
                    request.edge.render(decoded, request.treatment),
                    original,
                ))
            });
            if cancelled.load(Ordering::Acquire) {
                None
            } else {
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_discards_resident_variants_and_prevents_old_loads_from_being_reused() {
        let profile = tempfile::tempdir().unwrap();
        let url = "https://i.scdn.co/image/cache-clear-fixture";
        let path = disk::path(profile.path(), url, None).unwrap();
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([32, 64, 128, 255]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let image = Image::from_bytes(gpui::ImageFormat::Png, bytes.into_inner());
        let content = disk::image_content(&image).unwrap();
        disk::save(&path, &image);
        let store = Store::open(profile.path()).unwrap();

        let request = ArtworkRequest {
            source: ArtworkSource::new(url, false),
            edge: ArtworkEdge::new(40., 1.),
            treatment: decode::Treatment::Original,
        };
        let context = gpui::TestAppContext::single();
        context.update(|cx| {
            cx.set_global(ArtworkCache::new(Some(profile.path().to_owned())));
            assert!(cx.fetch_asset::<Artwork>(&request).is_none());
            cx.default_global::<ArtworkCache>().touch(
                request.clone(),
                request.edge.byte_bound(),
                None,
            );
        });
        context.run_until_parked();
        let version = super::super::colors::ALGORITHM_VERSION;
        assert!(
            store
                .artwork_palette(&content, version, 0)
                .unwrap()
                .is_some()
        );
        let old = context.update(|cx| {
            let artwork = cx.fetch_asset::<Artwork>(&request).flatten().unwrap();
            assert!(artwork.colors.is_some());
            let old = Arc::clone(&cx.default_global::<ArtworkCache>().cancelled);
            clear_artwork_cache(cx);
            assert_eq!(artwork_clear_status(cx), ArtworkClearStatus::Clearing);
            assert!(old.load(Ordering::Acquire));
            assert!(!cx.has_asset::<Artwork>(&request));
            let cache = cx.default_global::<ArtworkCache>();
            assert!(cache.originals.is_empty());
            assert_eq!(cache.decoded_bytes, 0);
            old
        });
        context.run_until_parked();
        context.update(|cx| {
            assert_eq!(artwork_clear_status(cx), ArtworkClearStatus::Cleared);
            assert!(old.load(Ordering::Acquire));
            assert!(
                !cx.default_global::<ArtworkCache>()
                    .cancelled
                    .load(Ordering::Acquire)
            );
            assert!(!cx.has_asset::<Artwork>(&request));
        });
        assert!(
            store
                .artwork_palette(&content, version, 0)
                .unwrap()
                .is_none()
        );
        assert_eq!(artwork_disk_usage(profile.path()).unwrap(), 0);
    }

    #[test]
    fn decoded_budget_reserves_pending_large_images_and_distinguishes_sizes() {
        let mut cache = ArtworkCache::new(None);
        let source = |id| ArtworkSource::new(&format!("https://i.scdn.co/image/{id}"), true);
        let large = ArtworkEdge::new(1500., 1.);
        let small = ArtworkEdge::new(40., 1.);
        let first = ArtworkRequest {
            source: source(0),
            edge: small,
            treatment: decode::Treatment::Original,
        };
        assert!(
            cache
                .touch(first.clone(), small.byte_bound(), None)
                .is_empty()
        );
        for id in 0..3 {
            let request = ArtworkRequest {
                source: source(id),
                edge: large,
                treatment: decode::Treatment::Original,
            };
            cache.touch(request, large.byte_bound(), None);
        }
        assert_eq!(cache.recent.len(), 2);
        assert!(!cache.recent.iter().any(|entry| entry.request == first));
        assert!(cache.decoded_bytes <= DECODED_BYTE_BUDGET);
    }
}
