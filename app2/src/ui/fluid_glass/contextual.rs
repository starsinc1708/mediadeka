//! Contextual pattern plan, palette light and the transition between them.

use std::time::Duration;

use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::PlaylistKey,
    ui::components::artwork::ArtworkColors,
    visual::{PatternId, PatternParams, PatternPlan, PatternSelection, VisualSettings},
};

use crate::ui::design_system::motion;

use super::{
    artwork::Seed,
    palette::{Cover, Field, Palette},
};

const PATTERN_CHANGE: Duration = Duration::from_millis(380);
const LIGHT_CHANGE: Duration = Duration::from_millis(620);
const PAGE_REFLECTION: f32 = 0.17;

const HOME: [PatternId; 3] = [
    PatternId::ThermalFlow,
    PatternId::Swirl,
    PatternId::GemSmoke,
];
const ARTIST: [PatternId; 3] = [
    PatternId::ThermalFlow,
    PatternId::LightLeak,
    PatternId::GemSmoke,
];
const ALBUM: [PatternId; 3] = [
    PatternId::Swirl,
    PatternId::ThermalFlow,
    PatternId::OpticalCaustics,
];
const PLAYLIST: [PatternId; 3] = [
    PatternId::Interference,
    PatternId::ThermalFlow,
    PatternId::Swirl,
];
const HISTORY: [PatternId; 2] = [PatternId::Interference, PatternId::LightLeak];
const LIBRARY: [PatternId; 2] = [PatternId::LightLeak, PatternId::ThermalFlow];
const SEARCH: [PatternId; 1] = [PatternId::LightLeak];
const SETTINGS: [PatternId; 1] = [PatternId::LightLeak];
const LYRICS: [PatternId; 3] = [
    PatternId::LightLeak,
    PatternId::LiquidMetal,
    PatternId::GemSmoke,
];
const FM_PATTERNS: [PatternId; 1] = [PatternId::ThermalFlow];

#[derive(Clone, Copy)]
pub(in crate::ui) enum Ambient {
    Home,
    Search,
    History,
    Library,
}

/// Matches the artwork placement decoded by the composite shader.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ArtworkPlacement {
    #[default]
    None = 0,
    Cover = 1,
    Portrait = 2,
}

impl ArtworkPlacement {
    fn for_object(kind: ObjectKind) -> Self {
        match kind {
            ObjectKind::Album | ObjectKind::Playlist | ObjectKind::Collection => Self::Cover,
            ObjectKind::Artist => Self::Portrait,
            ObjectKind::Station | ObjectKind::Track => Self::None,
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct PageAtmosphere<'a> {
    plan: PatternPlan,
    artwork: Option<&'a str>,
    placement: ArtworkPlacement,
    hero_light: f32,
}

impl<'a> PageAtmosphere<'a> {
    pub(in crate::ui) fn with_hero_light(mut self, strength: f32) -> Self {
        self.hero_light = strength;
        self
    }

    pub(in crate::ui) fn object(object: &ObjectRef, artwork: Option<&'a str>) -> Self {
        let seed = Seed::scene(object.kind.path(), &[object.provider.as_str(), &object.id]);
        Self::for_object(object.kind, seed, artwork)
    }

    pub(in crate::ui) fn local(
        kind: ObjectKind,
        identity: &[&str],
        artwork: Option<&'a str>,
    ) -> Self {
        Self::for_object(kind, Seed::scene(kind.path(), identity), artwork)
    }

    pub(in crate::ui) fn playlist(key: &PlaylistKey, artwork: Option<&'a str>) -> Self {
        Self::for_object(ObjectKind::Playlist, Seed::collection(key), artwork)
    }

    fn for_object(kind: ObjectKind, seed: Seed, artwork: Option<&'a str>) -> Self {
        let patterns = match kind {
            ObjectKind::Artist => &ARTIST[..],
            ObjectKind::Album => &ALBUM[..],
            ObjectKind::Playlist | ObjectKind::Collection => &PLAYLIST[..],
            ObjectKind::Station => &HOME[..],
            ObjectKind::Track => &LIBRARY[..],
        };
        Self {
            plan: PatternPlan::new(seed.pattern_seed(), patterns),
            artwork,
            placement: ArtworkPlacement::for_object(kind),
            hero_light: 1.,
        }
    }

    pub(in crate::ui) fn ambient(ambient: Ambient) -> Self {
        let (seed, patterns) = match ambient {
            Ambient::Home => (0x10, &HOME[..]),
            Ambient::Search => (0x20, &SEARCH[..]),
            Ambient::History => (0x30, &HISTORY[..]),
            Ambient::Library => (0x70, &LIBRARY[..]),
        };
        Self {
            plan: PatternPlan::new(seed, patterns),
            artwork: None,
            placement: ArtworkPlacement::None,
            hero_light: 1.,
        }
    }

    pub(in crate::ui) fn settings() -> Self {
        Self {
            plan: PatternPlan::new(0x50, &SETTINGS),
            artwork: None,
            placement: ArtworkPlacement::None,
            hero_light: 1.,
        }
    }

    pub(in crate::ui) fn lyrics() -> Self {
        Self {
            plan: PatternPlan::new(0x60, &LYRICS),
            artwork: None,
            placement: ArtworkPlacement::None,
            hero_light: 1.,
        }
    }
}

impl Default for PatternMix {
    fn default() -> Self {
        let params = PatternParams::for_plan(PatternId::ThermalFlow, 0, 0.65, 0.35);
        Self {
            a: PatternId::ThermalFlow,
            b: PatternId::ThermalFlow,
            mix: 0.,
            params,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct PatternMix {
    pub(in crate::ui) a: PatternId,
    pub(in crate::ui) b: PatternId,
    pub(in crate::ui) mix: f32,
    pub(in crate::ui) params: PatternParams,
}

pub(super) struct Contextual {
    cover: Cover,
    placement: ArtworkPlacement,
    hero_light: f32,
    plan: PatternPlan,
    track_variant: bool,
    track_changed: bool,
    pattern: PatternTransition,
    light: Transition<Option<PageLight>>,
}

impl Default for Contextual {
    fn default() -> Self {
        Self {
            cover: Cover::empty(),
            placement: ArtworkPlacement::None,
            hero_light: 1.,
            plan: PatternPlan::new(0, &FM_PATTERNS),
            track_variant: false,
            track_changed: false,
            pattern: PatternTransition::default(),
            light: Transition::default(),
        }
    }
}

impl Contextual {
    pub(super) fn differs(&self, page: Option<PageAtmosphere<'_>>) -> bool {
        self.plan != page.map_or_else(|| PatternPlan::new(0, &FM_PATTERNS), |page| page.plan)
            || self.cover.url() != page.and_then(|page| page.artwork)
            || self.placement != page.map_or(ArtworkPlacement::None, |page| page.placement)
            || self.hero_light != page.map_or(1., |page| page.hero_light)
    }

    pub(super) fn select(&mut self, page: Option<PageAtmosphere<'_>>) {
        let plan = page.map_or_else(|| PatternPlan::new(0, &FM_PATTERNS), |page| page.plan);
        if self.plan != plan {
            self.plan = plan;
            self.track_changed = false;
        }
        self.placement = page.map_or(ArtworkPlacement::None, |page| page.placement);
        self.hero_light = page.map_or(1., |page| page.hero_light);
        if self.cover.select(page.and_then(|page| page.artwork)) {
            self.light.select(None);
        }
    }

    pub(super) fn select_track(&mut self, id: Option<&str>) {
        let variant = id.is_some_and(|id| Seed::scene("playing", &[id]).pattern_seed() & 1 != 0);
        self.track_changed |= self.track_variant != variant;
        self.track_variant = variant;
    }

    pub(super) fn artwork_url(&self) -> Option<&str> {
        self.cover.url()
    }

    pub(super) fn resolve(
        &mut self,
        revision: Option<[u8; 32]>,
        colors: impl FnOnce(&str) -> Option<Option<ArtworkColors>>,
    ) {
        if let Some(colors) = self.cover.resolve(revision, colors) {
            self.light.select(colors.map(|colors| PageLight {
                palette: Palette::page(colors),
                amount: 1.,
            }));
        }
    }

    /// Drops the artwork light when the user chooses a fixed palette. Auto
    /// resolves the current cover again on the next frame.
    pub(super) fn clear_light(&mut self) {
        self.light.select(None);
    }

    pub(super) fn auto_pattern(&self, follow_track: bool) -> PatternId {
        let default = self.plan.selected().unwrap_or(PatternId::ThermalFlow);
        if follow_track
            && self.track_variant
            && self.placement == ArtworkPlacement::None
            && let Some(alternate) = self
                .plan
                .allowed
                .iter()
                .copied()
                .find(|&pattern| pattern != default)
        {
            alternate
        } else {
            default
        }
    }

    pub(super) fn plan_seed(&self) -> u32 {
        self.plan.seed
    }

    pub(super) fn artwork_placement(&self) -> ArtworkPlacement {
        self.placement
    }

    pub(super) fn hero_light(&self) -> f32 {
        self.hero_light
    }

    #[cfg(feature = "capture")]
    pub(super) fn palette(&self) -> Option<Palette> {
        self.light.target.map(|light| light.palette)
    }

    pub(super) fn sample(
        &mut self,
        now: Duration,
        motion: bool,
        mut field: Field,
        settings: VisualSettings,
        follow_track: bool,
    ) -> (PatternMix, Field) {
        let base = self.plan.selected().unwrap_or(PatternId::ThermalFlow);
        let track_changed = std::mem::take(&mut self.track_changed);
        let (target, basis, duration) = match settings.pattern {
            PatternSelection::Auto => (
                self.auto_pattern(follow_track),
                base,
                if follow_track && track_changed {
                    motion::MOTION.track_pattern()
                } else {
                    PATTERN_CHANGE
                },
            ),
            PatternSelection::Fixed(pattern) => (pattern, pattern, PATTERN_CHANGE),
        };
        self.pattern.select(target, duration);
        let (a, b, mix) = self.pattern.sample(now, motion);
        let params =
            PatternParams::for_plan(basis, self.plan.seed, settings.intensity, settings.speed);
        if let Some(light) = self.light.sample(now, motion, LIGHT_CHANGE, mix_light) {
            field.upper = field.upper.interpolate(light.palette, light.amount);
            if self.placement == ArtworkPlacement::None {
                field.player = field
                    .player
                    .interpolate(light.palette, light.amount * PAGE_REFLECTION);
            }
        }
        (PatternMix { a, b, mix, params }, field)
    }
}

#[derive(Default)]
struct PatternTransition {
    from: PatternId,
    to: PatternId,
    target: PatternId,
    progress: f32,
    from_progress: f32,
    duration: Duration,
    started: Option<Duration>,
}

impl PatternTransition {
    fn select(&mut self, target: PatternId, duration: Duration) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.duration = duration;
        self.started = None;
        if target == self.from && self.from != self.to {
            std::mem::swap(&mut self.from, &mut self.to);
            self.progress = 1. - self.progress;
            self.from_progress = self.progress;
            return;
        }
        let current = if self.progress >= 0.5 {
            self.to
        } else {
            self.from
        };
        self.from = current;
        self.to = target;
        self.progress = 0.;
        self.from_progress = 0.;
    }

    fn sample(&mut self, now: Duration, motion: bool) -> (PatternId, PatternId, f32) {
        if !motion {
            self.from = self.target;
            self.to = self.target;
            self.progress = 1.;
            self.started = None;
            return (self.from, self.to, 0.);
        }
        if self.from == self.to {
            self.progress = 1.;
            return (self.from, self.to, 0.);
        }
        let started = *self.started.get_or_insert(now);
        let progress = now.saturating_sub(started).as_secs_f32() / self.duration.as_secs_f32();
        if progress >= 1. {
            self.from = self.to;
            self.progress = 1.;
            self.started = None;
            return (self.from, self.to, 0.);
        }
        self.progress =
            self.from_progress + (1. - self.from_progress) * motion::smoothstep(progress);
        (self.from, self.to, self.progress)
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PageLight {
    palette: Palette,
    amount: f32,
}

fn mix_light(from: Option<PageLight>, to: Option<PageLight>, progress: f32) -> Option<PageLight> {
    match (from, to) {
        (Some(from), Some(to)) => Some(PageLight {
            palette: from.palette.interpolate(to.palette, progress),
            amount: from.amount + (to.amount - from.amount) * progress,
        }),
        (Some(light), None) => Some(PageLight {
            amount: light.amount * (1. - progress),
            ..light
        }),
        (None, Some(light)) => Some(PageLight {
            amount: light.amount * progress,
            ..light
        }),
        (None, None) => None,
    }
}

#[derive(Default)]
struct Transition<T> {
    painted: T,
    target: T,
    current: Option<(T, Option<Duration>)>,
}

impl<T: Copy + PartialEq> Transition<T> {
    fn select(&mut self, target: T) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.current = (self.painted != target).then_some((self.painted, None));
    }

    fn sample(
        &mut self,
        now: Duration,
        animated: bool,
        duration: Duration,
        mix: impl FnOnce(T, T, f32) -> T,
    ) -> T {
        if animated && let Some((from, start)) = &mut self.current {
            let elapsed = now.saturating_sub(*start.get_or_insert(now));
            let progress = elapsed.as_secs_f32() / duration.as_secs_f32();
            if progress < 1. {
                self.painted = mix(*from, self.target, motion::smoothstep(progress));
                return self.painted;
            }
        }
        self.current = None;
        self.painted = self.target;
        self.painted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderId;

    #[test]
    fn object_identity_selects_a_stable_pattern_from_its_page_family() {
        for id in 0..128 {
            let album = ObjectRef {
                provider: ProviderId::Spotify,
                kind: ObjectKind::Album,
                id: id.to_string(),
            };
            let first = PageAtmosphere::object(&album, None);
            let second = PageAtmosphere::object(&album, Some("changed"));
            assert_eq!(first.plan, second.plan);
            assert!(matches!(
                first.plan.selected(),
                Some(PatternId::Swirl | PatternId::ThermalFlow | PatternId::OpticalCaustics)
            ));
            let artist = ObjectRef {
                kind: ObjectKind::Artist,
                ..album
            };
            let artist_plan = PageAtmosphere::object(&artist, None);
            assert!(matches!(
                artist_plan.plan.selected(),
                Some(PatternId::ThermalFlow | PatternId::LightLeak | PatternId::GemSmoke)
            ));
        }
    }

    #[test]
    fn clearing_page_light_removes_the_artwork_tint_for_a_fixed_palette() {
        let mut contextual = Contextual::default();
        contextual.light.select(Some(PageLight {
            palette: crate::appearance::GlassPalette::Abyss.palette(None),
            amount: 1.,
        }));
        contextual.clear_light();
        let field = Field::uniform(crate::appearance::GlassPalette::Solar.palette(None));
        let (_, sampled) = contextual.sample(
            Duration::ZERO,
            false,
            field,
            VisualSettings::standard(),
            false,
        );
        assert_eq!(sampled, field);
    }

    #[test]
    fn a_retargeted_pattern_keeps_the_dominant_visible_pattern_and_settles() {
        let mut transition = PatternTransition::default();
        transition.select(PatternId::Swirl, PATTERN_CHANGE);
        assert_eq!(
            transition.sample(Duration::ZERO, true),
            (PatternId::ThermalFlow, PatternId::Swirl, 0.)
        );
        let middle = transition.sample(PATTERN_CHANGE / 2, true);
        assert!(middle.2 > 0. && middle.0 == PatternId::ThermalFlow);
        transition.select(PatternId::Swirl, PATTERN_CHANGE);
        assert_eq!(transition.sample(PATTERN_CHANGE / 2, true), middle);
        transition.select(PatternId::LiquidMetal, PATTERN_CHANGE);
        assert_eq!(
            transition.sample(PATTERN_CHANGE, false),
            (PatternId::LiquidMetal, PatternId::LiquidMetal, 0.)
        );
    }

    #[test]
    fn auto_track_pattern_reverses_from_the_visible_mix_and_keeps_page_artwork() {
        let mut contextual = Contextual::default();
        contextual.select(Some(PageAtmosphere::ambient(Ambient::Library)));
        let field = Field::uniform(crate::appearance::GlassPalette::Solar.palette(None));
        let visual = VisualSettings::standard();
        let (before, _) = contextual.sample(Duration::ZERO, false, field, visual, true);
        let id = (0..64)
            .map(|i| i.to_string())
            .find(|id| Seed::scene("playing", &[id]).pattern_seed() & 1 != 0)
            .unwrap();
        let interval = motion::MOTION.track_pattern();
        contextual.select_track(Some(&id));
        assert_eq!(contextual.auto_pattern(false), before.a);
        contextual.sample(Duration::ZERO, true, field, visual, true);
        let (middle, _) = contextual.sample(interval / 2, true, field, visual, true);
        assert_eq!(middle.a, before.a);
        assert_ne!(middle.a, middle.b);
        assert_eq!(middle.mix, 0.5);
        assert_eq!(middle.params, before.params);
        contextual.select_track(None);
        let (reversed, _) = contextual.sample(interval / 2, true, field, visual, true);
        assert_eq!((reversed.a, reversed.b), (middle.b, middle.a));
        assert_eq!(reversed.mix, 1. - middle.mix);
        let (after, _) = contextual.sample(interval * 2, true, field, visual, true);
        assert_eq!(after, before);

        let album = PageAtmosphere::local(ObjectKind::Album, &["shared"], None);
        contextual.select(Some(album));
        let (album_pattern, _) = contextual.sample(Duration::ZERO, false, field, visual, true);
        contextual.select_track(Some(&id));
        let (with_music, _) = contextual.sample(interval, true, field, visual, true);
        assert_eq!(with_music, album_pattern);
    }
}
