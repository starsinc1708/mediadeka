use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Div, ObjectFit, RenderImage, SharedString, Styled, Window, div, img,
    prelude::*, px,
};

use crate::ui::{
    components::{artwork, music},
    design_system::motion::smoothstep,
    fluid_glass::{
        artwork::{self as fluid_artwork, Seed},
        track_change::{COVER_DELAY, COVER_FADE},
    },
};

const CORNER_RADIUS: f32 = 8.;

pub(super) struct Source<'a> {
    pub revision: usize,
    pub track: Option<usize>,
    pub url: Option<&'a str>,
    pub fallback: Option<Seed>,
    pub title: &'a str,
    pub artist: &'a str,
}

#[derive(Clone)]
enum Cover {
    Empty,
    Pattern(Seed),
    Image(Arc<RenderImage>),
}

impl Cover {
    fn layer(&self, opacity: f32) -> Option<AnyElement> {
        match self {
            Self::Empty => None,
            Self::Pattern(seed) => {
                Some(fluid_artwork::pattern(*seed, px(CORNER_RADIUS), opacity).into_any_element())
            }
            Self::Image(image) => Some(
                img(image.clone())
                    .absolute()
                    .inset_0()
                    .size_full()
                    .rounded(px(CORNER_RADIUS))
                    .object_fit(ObjectFit::Cover)
                    .opacity(opacity)
                    .into_any_element(),
            ),
        }
    }
}

type Selection = (Option<usize>, usize, Option<Seed>);

#[derive(Clone)]
struct Layer {
    cover: Cover,
    title: SharedString,
    artist: SharedString,
}

impl Layer {
    fn new(cover: Cover, title: &str, artist: &str) -> Self {
        Self {
            cover,
            title: SharedString::from(title),
            artist: SharedString::from(artist),
        }
    }
}

pub(super) struct Frame {
    pub cover: Div,
    pub labels: Vec<(SharedString, SharedString, f32)>,
}

pub(super) struct FluidCover {
    origin: Instant,
    configured: bool,
    selection: Option<Selection>,
    pending_since: Option<Duration>,
    displayed: Vec<(Layer, f32)>,
    transition: Option<Transition>,
    #[cfg(feature = "capture")]
    sample: Option<Duration>,
}

struct Transition {
    next: Layer,
    started: Duration,
}

impl Transition {
    fn progress(&self, now: Duration) -> f32 {
        (now.saturating_sub(self.started).as_secs_f32() / COVER_FADE.as_secs_f32()).min(1.)
    }
}

impl FluidCover {
    pub(super) fn image_for_revision(&self, revision: usize) -> Option<Arc<RenderImage>> {
        if self
            .selection
            .is_none_or(|(_, selected, _)| selected != revision)
            || self.pending_since.is_some()
        {
            return None;
        }
        let layer = if let Some(transition) = &self.transition {
            &transition.next
        } else {
            &self.displayed.last()?.0
        };
        match &layer.cover {
            Cover::Image(image) => Some(image.clone()),
            Cover::Empty | Cover::Pattern(_) => None,
        }
    }

    pub(super) fn new(configured: bool) -> Self {
        Self {
            origin: Instant::now(),
            configured,
            selection: None,
            pending_since: None,
            displayed: Vec::new(),
            transition: None,
            #[cfg(feature = "capture")]
            sample: None,
        }
    }

    #[cfg(feature = "capture")]
    pub(super) fn sample(&mut self, time: Duration) {
        self.sample = Some(time);
    }

    #[cfg(feature = "capture")]
    pub(super) fn labels_for_capture(&self) -> Vec<(String, String, f32)> {
        self.visible(self.sample.unwrap_or_else(|| self.origin.elapsed()))
            .into_iter()
            .map(|(layer, weight)| (layer.title.to_string(), layer.artist.to_string(), weight))
            .collect()
    }

    pub(super) fn render(
        &mut self,
        source: Source<'_>,
        edge: f32,
        reduced_motion: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        #[cfg(feature = "capture")]
        let sample = self.sample;
        #[cfg(not(feature = "capture"))]
        let sample: Option<Duration> = None;
        let now = sample.unwrap_or_else(|| self.origin.elapsed());
        let animated = self.configured
            && !reduced_motion
            && !cx.reduce_motion()
            && (sample.is_some() || (window.is_window_active() && window.is_visible()));
        let fallback = source
            .fallback
            .filter(|_| fluid_artwork::optics_enabled(cx));
        let decoded = source
            .url
            .map_or(Some(None), |url| {
                artwork::decoded_cover(url, px(edge), window, cx)
            })
            .map(|image| {
                image.map_or_else(
                    || fallback.map_or(Cover::Empty, Cover::Pattern),
                    Cover::Image,
                )
            });
        let selected_seed = match &decoded {
            Some(Cover::Pattern(seed)) => Some(*seed),
            // Resizing may request a different image bucket of the same cover.
            None => self
                .selection
                .filter(|(_, revision, _)| *revision == source.revision)
                .and_then(|(_, _, seed)| seed),
            _ => None,
        };
        let selection = (source.track, source.revision, selected_seed);
        let decoded =
            decoded.or_else(|| (!animated).then(|| fallback.map_or(Cover::Empty, Cover::Pattern)));
        if self.update(
            selection,
            now,
            animated,
            decoded.map(|cover| Layer::new(cover, source.title, source.artist)),
        ) && sample.is_none()
        {
            window.request_animation_frame();
        }
        let visible = self.visible(now);
        let mut remaining = 1.;
        let mut covers = Vec::new();
        // Convert the frozen contribution of each cover back to source-over
        // alpha. Retargeting can keep a composite without reading back pixels.
        for (layer, weight) in visible.iter().rev() {
            let opacity = (*weight / f32::max(remaining, f32::EPSILON)).clamp(0., 1.);
            remaining -= weight;
            covers.extend(layer.cover.layer(opacity));
        }
        covers.reverse();
        Frame {
            cover: music::fluid_cover_slot(edge, CORNER_RADIUS)
                .child(div().absolute().inset_0().children(covers)),
            labels: visible
                .into_iter()
                .map(|(layer, weight)| (layer.title, layer.artist, weight))
                .collect(),
        }
    }

    fn update(
        &mut self,
        selection: Selection,
        now: Duration,
        animated: bool,
        decoded: Option<Layer>,
    ) -> bool {
        let new_track = self
            .selection
            .is_none_or(|current| current.0 != selection.0 || current.1 != selection.1);
        if new_track {
            self.displayed = self.visible(now);
            self.transition = None;
            self.pending_since = Some(now);
        }
        self.selection = Some(selection);

        if let Some(layer) = decoded {
            match self.pending_since {
                Some(since) if animated && !self.displayed.is_empty() => {
                    if now.saturating_sub(since) < COVER_DELAY {
                        return true;
                    }
                    self.transition = Some(Transition {
                        next: layer,
                        started: now,
                    });
                    self.pending_since = None;
                }
                _ => {
                    self.pending_since = None;
                    if let Some(transition) = &mut self.transition {
                        transition.next = layer;
                    } else {
                        self.displayed = vec![(layer, 1.)];
                    }
                }
            }
        }

        if let Some(transition) = &self.transition {
            if animated && transition.progress(now) < 1. {
                return true;
            }
            self.displayed = vec![(self.transition.take().expect("active transition").next, 1.)];
        }
        false
    }

    fn visible(&self, now: Duration) -> Vec<(Layer, f32)> {
        let Some(transition) = &self.transition else {
            return self.displayed.clone();
        };
        let amount = smoothstep(transition.progress(now));
        self.displayed
            .iter()
            .map(|(layer, weight)| (layer.clone(), weight * (1. - amount)))
            .chain(std::iter::once((transition.next.clone(), amount)))
            .filter(|(_, opacity)| *opacity > 0.)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Frame, RgbaImage};

    fn track(id: usize) -> Selection {
        (Some(id), 0, None)
    }

    fn cover(name: &str) -> Layer {
        let image = Arc::new(RenderImage::new([Frame::new(RgbaImage::new(1, 1))]));
        Layer::new(Cover::Image(image), name, name)
    }

    #[test]
    fn slow_decode_keeps_the_old_image_without_scheduling_frames() {
        let ms = Duration::from_millis;
        let mut player = FluidCover::new(true);
        assert!(!player.update(track(1), ms(0), false, Some(cover("old"))));
        assert!(!player.update(track(2), ms(50), true, None));
        assert!(!player.update(track(2), ms(900), true, None));
        assert_eq!(player.visible(ms(900))[0].0.title, "old");
        assert!(player.update(track(2), ms(950), true, Some(cover("new"))));
        let middle = player.visible(ms(1070));
        assert_eq!((middle[0].1, middle[1].1), (0.5, 0.5));
        assert_eq!((&*middle[0].0.title, &*middle[1].0.title), ("old", "new"));
        assert!(!player.update(track(2), ms(1190), true, Some(cover("new"))));
        assert_eq!(player.visible(ms(1190))[0].0.title, "new");
    }

    #[test]
    fn reduction_ends_a_fade_and_a_failed_decode_reveals_the_placeholder() {
        let ms = Duration::from_millis;
        let mut player = FluidCover::new(true);
        player.update(track(1), ms(0), false, Some(cover("old")));
        let no_cover = Some(Layer::new(Cover::Empty, "new", "artist"));
        player.update(track(2), ms(50), true, no_cover.clone());
        assert!(player.update(track(2), ms(300), true, no_cover.clone()));
        assert!(matches!(
            player.visible(ms(300))[0].0.cover,
            Cover::Image(_)
        ));
        assert!(!player.update(track(2), ms(305), false, no_cover));
        assert!(player.transition.is_none());
        assert!(matches!(player.displayed[0].0.cover, Cover::Empty));
    }

    #[test]
    fn successive_missing_covers_transition_with_the_same_url_revision() {
        let ms = Duration::from_millis;
        let mut player = FluidCover::new(true);
        let first = Seed::playlist(1);
        let next = Seed::playlist(2);
        player.update(
            track(1),
            ms(0),
            false,
            Some(Layer::new(Cover::Pattern(first), "old", "")),
        );
        let target = Some(Layer::new(Cover::Pattern(next), "new", ""));
        assert!(player.update(track(2), ms(50), true, target.clone()));
        assert!(matches!(player.visible(ms(50))[0].0.cover, Cover::Pattern(seed) if seed == first));
        assert!(player.update(track(2), ms(300), true, target));
        assert!(matches!(player.visible(ms(540))[0].0.cover, Cover::Pattern(seed) if seed == next));
    }

    #[test]
    fn a_third_track_starts_from_the_visible_cover_and_labels() {
        let ms = Duration::from_millis;
        let mut player = FluidCover::new(true);
        player.update(track(1), ms(0), false, Some(cover("first")));
        player.update(track(2), ms(10), true, Some(cover("second")));
        player.update(track(2), ms(260), true, Some(cover("second")));
        assert!(player.update(track(3), ms(380), true, Some(cover("third"))));
        let middle = player.visible(ms(380));
        assert_eq!(
            middle.iter().map(|(_, weight)| *weight).collect::<Vec<_>>(),
            [0.5, 0.5]
        );
        assert_eq!(
            (&*middle[0].0.title, &*middle[1].0.title),
            ("first", "second")
        );
        assert!(player.update(track(3), ms(630), true, Some(cover("third"))));
        let retargeted = player.visible(ms(750));
        assert_eq!(
            retargeted
                .iter()
                .map(|(_, weight)| *weight)
                .collect::<Vec<_>>(),
            [0.25, 0.25, 0.5]
        );
        assert_eq!(&*retargeted[2].0.title, "third");
    }
}
