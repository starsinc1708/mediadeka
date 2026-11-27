//! Stable, decorative artwork for media that has no decoded cover.

use std::sync::LazyLock;

use gpui::{
    App, Bounds, Corners, Global, IntoElement, Pixels, Shader, ShaderError, ShaderInstance,
    ShaderSource, Styled, Window, canvas, px, rgb,
};

use crate::{
    music::{
        ObjectRef,
        library::{EntryAction, LibraryGroup},
    },
    playlist::{Media, PlaylistKey, PlaylistTrack},
    ui::components::artwork,
};

use super::{effects_enabled, tokens};

mod texture;
pub(in crate::ui) use texture::cached_cover;

/// Keep one decoded representation as the Hero passes through size buckets.
pub(in crate::ui) const HERO_IMAGE_EDGE: Pixels = px(340.);

const PALETTES: [[u32; 3]; 4] = [
    [
        tokens::SOLAR_ORANGE,
        tokens::ELECTRIC_CYAN,
        tokens::WARM_LIGHT,
    ],
    [0x8969d6, tokens::ELECTRIC_CYAN, tokens::ACCENT],
    [
        tokens::HOT_MAGENTA,
        tokens::DEEP_BLUE,
        tokens::ELECTRIC_CYAN,
    ],
    [0x75819e, 0x5a497b, 0xbecfe0],
];

static SHADER: LazyLock<Result<Shader, ShaderError>> = LazyLock::new(|| {
    let shader = Shader::new(ShaderSource::new(
        "Mediadeka Fluid Glass artwork",
        include_str!("graphics/cover.wgsl"),
    ));
    if let Err(error) = &shader {
        eprintln!("Не удалось зарегистрировать обложки Fluid Glass: {error}");
    }
    shader
});

struct ArtworkOptics(bool);

impl Global for ArtworkOptics {}

pub(in crate::ui) fn configure(cx: &mut App, high_contrast: bool) {
    cx.set_global(ArtworkOptics(!high_contrast && effects_enabled()));
}

pub(in crate::ui) fn optics_enabled(cx: &App) -> bool {
    cx.try_global::<ArtworkOptics>().is_some_and(|mode| mode.0)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) struct Seed(u64);

impl Seed {
    pub(in crate::ui) fn entry(entry: &EntryAction) -> Option<Self> {
        match entry {
            EntryAction::Track(_, track) => Self::track(track),
            EntryAction::LocalGroup(LibraryGroup::Album { title, artist }) => Some(
                Self::from_parts(b"album", [title.as_bytes(), artist.as_bytes()]),
            ),
            EntryAction::LocalPlaylist { id, .. } => Some(Self::playlist(*id)),
            _ => None,
        }
    }

    pub(in crate::ui) fn track(track: &PlaylistTrack) -> Option<Self> {
        (!track.id.is_empty() && matches!(track.media, Some(Media::File(_))))
            .then(|| Self::from_parts(b"track", [track.id.as_bytes()]))
    }

    pub(in crate::ui) fn queue_track(key: &PlaylistKey, track: &PlaylistTrack) -> Self {
        Self::track(track).unwrap_or_else(|| {
            let provider = key
                .provider()
                .map_or("local", |provider| provider.display_name());
            Self::scene(
                "queue-track",
                &[provider, &track.id, &track.artist, &track.title],
            )
        })
    }

    pub(in crate::ui) fn playlist(id: i64) -> Self {
        Self::from_parts(b"playlist", [id.to_le_bytes().as_slice()])
    }

    pub(in crate::ui) fn object(reference: &ObjectRef) -> Self {
        Self::scene(
            reference.kind.path(),
            &[reference.provider.as_str(), &reference.id],
        )
    }

    pub(in crate::ui) fn collection(key: &PlaylistKey) -> Self {
        match key {
            PlaylistKey::Local(id) => Self::playlist(*id),
            PlaylistKey::Source { provider, id, .. } => {
                Self::scene("playlist", &[provider.as_str(), id])
            }
            PlaylistKey::Station { provider, id, .. } => {
                Self::scene("station", &[provider.as_str(), id])
            }
        }
    }

    pub(super) fn scene(kind: &str, parts: &[&str]) -> Self {
        Self::from_parts(kind.as_bytes(), parts.iter().map(|part| part.as_bytes()))
    }

    fn from_parts<'a>(kind: &[u8], parts: impl IntoIterator<Item = &'a [u8]>) -> Self {
        // Fixed FNV-1a and explicit byte order keep existing artwork stable across launches.
        const OFFSET: u64 = 0xcbf29ce484222325;
        const PRIME: u64 = 0x100000001b3;
        let hash = |state: u64, bytes: &[u8]| {
            bytes.iter().fold(state, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
            })
        };
        let seed = parts.into_iter().fold(hash(OFFSET, kind), |state, part| {
            hash(hash(state, &(part.len() as u64).to_le_bytes()), part)
        });
        // Mix all identity bits into the family and its phase. Nearby paths
        // should not converge on one visual family because of a common prefix.
        let seed = (seed ^ (seed >> 33)).wrapping_mul(0xff51afd7ed558ccd);
        let seed = (seed ^ (seed >> 33)).wrapping_mul(0xc4ceb9fe1a85ec53);
        Self(seed ^ (seed >> 33))
    }

    pub(super) fn unit(self, shift: u32) -> f32 {
        ((self.0 >> shift) & 0xffff_u64) as f32 / 65_536.
    }

    pub(in crate::ui) fn pattern_seed(self) -> u32 {
        (self.0 ^ (self.0 >> 32)) as u32
    }

    fn family(self) -> usize {
        ((self.0 >> 54) & 3) as usize
    }

    fn instance(self) -> ShaderInstance {
        // Divide only 16-bit slices so every seed component is portable to HLSL.
        let family = self.family();
        let [primary, secondary, highlight] = PALETTES[family];
        ShaderInstance::default()
            .params(
                0,
                &[family as f32, self.unit(0), self.unit(16), self.unit(32)],
            )
            .color(0, rgb(primary))
            .color(1, rgb(secondary))
            .color(2, rgb(highlight))
            .color(3, rgb(tokens::CANVAS))
    }

    pub(in crate::ui) fn paint(
        self,
        bounds: Bounds<Pixels>,
        radius: Pixels,
        window: &mut Window,
        cx: &App,
    ) {
        if optics_enabled(cx)
            && let Ok(shader) = SHADER.as_ref()
        {
            window.paint_shader(bounds, Corners::all(radius), shader, self.instance());
        }
    }
}

pub(in crate::ui) fn thumbnail(
    url: Option<&str>,
    fallback: Option<Seed>,
    radius: Pixels,
    card: bool,
) -> impl IntoElement {
    artwork::remote_rounded_or(url, false, radius, card, move |bounds, window, cx| {
        cached_cover(fallback?, bounds.size.width, window, cx)
    })
}

pub(in crate::ui) fn hero_thumbnail(
    url: Option<&str>,
    fallback: Option<Seed>,
    radius: Pixels,
) -> impl IntoElement {
    artwork::remote_rounded_or_at_edge(
        url,
        false,
        radius,
        true,
        Some(HERO_IMAGE_EDGE),
        move |_, window, cx| cached_cover(fallback?, HERO_IMAGE_EDGE, window, cx),
    )
}

pub(in crate::ui) fn pattern(seed: Seed, radius: Pixels, opacity: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, cx| seed.paint(bounds, radius, window, cx),
    )
    .absolute()
    .inset_0()
    .size_full()
    .opacity(opacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_uses_kind_and_unambiguous_stable_identity() {
        let album = |title: &str, artist: &str| {
            Seed::entry(&EntryAction::LocalGroup(LibraryGroup::Album {
                title: title.into(),
                artist: artist.into(),
            }))
        };
        assert_eq!(album("Ёлки", "Би-2"), Some(Seed(0x54025c092ba750df)));
        assert_ne!(album("ab", "c"), album("a", "bc"));
        assert_ne!(Seed::playlist(1), Seed::playlist(2));
        assert_ne!(
            Seed::playlist(1),
            Seed::from_parts(b"track", [1_i64.to_le_bytes().as_slice()])
        );
    }

    #[cfg(windows)]
    #[test]
    fn artwork_shader_translates_to_direct3d() {
        let shader = SHADER.as_ref().expect("valid artwork WGSL");
        gpui::shader_program(shader.id())
            .expect("registered artwork shader")
            .to_hlsl()
            .expect("artwork translated to HLSL");
    }
}
