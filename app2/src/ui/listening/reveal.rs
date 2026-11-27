//! Staged reveal inside the shared Now Playing artwork transition.
//!
//! Every phase is a fraction of the same 520 ms cover motion. The formulas live
//! here so the view, the player and the native capture read one implementation.

use crate::ui::music::detail::hero_motion::HeroMotion;

pub(super) const METADATA_REST: f32 = 0.55;
pub(super) const METADATA_OFFSET: f32 = 8.;
const PROGRESS_REST: f32 = 0.10;
const PROGRESS_OFFSET: f32 = 4.;
const TRANSPORT_REST: f32 = 0.20;
const TRANSPORT_OFFSET: f32 = 3.;

/// Unit tests exercise the same phase window with a linear progress value.
#[cfg(test)]
pub(in crate::ui) fn metadata_at(progress: f32) -> (f32, f32) {
    let phase = phase_at(progress, 0., 0.55);
    (
        METADATA_REST + (1. - METADATA_REST) * phase,
        METADATA_OFFSET * (1. - phase),
    )
}

pub(in crate::ui) fn metadata(motion: Option<HeroMotion>) -> (f32, f32) {
    motion.map_or((1., 0.), |motion| {
        let phase = motion.phased(0., 0.55);
        (
            METADATA_REST + (1. - METADATA_REST) * phase,
            METADATA_OFFSET * (1. - phase),
        )
    })
}

pub(in crate::ui) fn progress(motion: Option<HeroMotion>) -> (f32, f32) {
    motion.map_or((1., 0.), |motion| {
        let phase = motion.phased(0.14, 0.66);
        (
            PROGRESS_REST + (1. - PROGRESS_REST) * phase,
            PROGRESS_OFFSET * (1. - phase),
        )
    })
}

pub(in crate::ui) fn transport(motion: Option<HeroMotion>) -> (f32, f32) {
    motion.map_or((1., 0.), |motion| {
        let phase = motion.phased(0.30, 0.86);
        (
            TRANSPORT_REST + (1. - TRANSPORT_REST) * phase,
            TRANSPORT_OFFSET * (1. - phase),
        )
    })
}

#[cfg(test)]
fn phase_at(progress: f32, start: f32, end: f32) -> f32 {
    if end <= start {
        return f32::from(u8::from(progress >= end));
    }
    crate::ui::design_system::motion::out_quad(((progress - start) / (end - start)).clamp(0., 1.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_enters_late_and_finishes_inside_the_shared_transition() {
        let metadata = metadata_at(0.);
        let progress = progress(None);
        let transport = transport(None);
        assert_eq!(metadata.0, METADATA_REST);
        assert_eq!(progress, (1., 0.));
        assert_eq!(transport, (1., 0.));
        assert_eq!(metadata_at(1.).0, 1.);
        assert_eq!(metadata_at(0.55), (1., 0.));
    }

    #[test]
    fn stages_are_ordered_metadata_progress_transport() {
        let at = |progress| {
            (
                metadata_at(progress).0,
                crate::ui::design_system::motion::out_quad(
                    ((progress - 0.14) / (0.66 - 0.14)).clamp(0., 1.),
                ),
                crate::ui::design_system::motion::out_quad(
                    ((progress - 0.30) / (0.86 - 0.30)).clamp(0., 1.),
                ),
            )
        };
        let (metadata, progress, transport) = at(0.42);
        assert!(metadata > progress && progress > transport);
        assert_eq!(at(1.), (1., 1., 1.));
    }
}
