#[path = "album/atmosphere.rs"]
mod atmosphere;
#[path = "album/motion.rs"]
pub(super) mod motion;

use std::{fs, io, path::Path};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use mediadeka::{
    MainView,
    playlist::{Media, PlaylistTrack},
};
use rusqlite::{Connection, params};
use serde_json::json;

use super::super::{Scene, frame, node_label, require, snapshot, wait};

const ID: &str = "FluidScrollAlbum001234";
pub(crate) const URL: &str = "https://open.spotify.com/album/FluidScrollAlbum001234";
pub(crate) const ART: &str = "https://i.scdn.co/image/fluid-scroll-album";
pub(crate) const TITLE: &str = "Течения подо льдом";
const HEADING: &str = "music-object-title";

pub(super) struct ShortRelease {
    name: &'static str,
    id: &'static str,
    title: &'static str,
    tracks: usize,
}

pub(super) const SHORT_RELEASES: [ShortRelease; 2] = [
    ShortRelease {
        name: "stage-single",
        id: "FluidShortSingle001234",
        title: "Ледяной берег",
        tracks: 1,
    },
    ShortRelease {
        name: "stage-ep",
        id: "FluidShortRecord001234",
        title: "У кромки воды",
        tracks: 4,
    },
];

impl ShortRelease {
    fn url(&self) -> String {
        format!("https://open.spotify.com/album/{}", self.id)
    }

    fn art(&self) -> String {
        format!("https://i.scdn.co/image/{}", self.name)
    }

    pub(super) fn item(&self) -> serde_json::Value {
        json!({
            "id": format!("spotify:album:{}", self.id),
            "title": self.title,
            "subtitle": "Северный ансамбль",
            "url": self.url(),
            "artwork": self.art(),
        })
    }
}

pub(super) fn seed(profile: &Path, region: &str) -> gpui::Result<()> {
    let cover = RgbImage::from_fn(640, 640, |x, y| {
        let x = x as f32 / 640.;
        let y = y as f32 / 640.;
        let ripple = (20. * (y - 0.29 * (x * 3.5 + y * 2.).sin())).sin();
        let bright = (1. - (x + y - 0.89).abs()).max(0.).powi(5);
        let late = (ripple.max(0.) * bright).powi(2);
        let core = 0.52 * bright + 0.48 * late;
        Rgb([11. + core * 49., 23. + core * 193., 38. + core * 171.].map(|value| value as u8))
    });
    let path = profile.join("fluid-scroll-album.cover");
    let mut bytes = io::Cursor::new(vec![2]); // PNG in the disk artwork cache.
    bytes.set_position(1);
    DynamicImage::ImageRgb8(cover).write_to(&mut bytes, ImageFormat::Png)?;
    fs::write(&path, bytes.into_inner())?;
    MainView::seed_artist_artwork_for_capture(profile, ART, &path)?;
    let tracks: Vec<_> = (1..=40)
        .map(|index| PlaylistTrack {
            id: format!("fluid-album-track-{index:02}"),
            title: format!("Течение {index:02}"),
            artist: "Северный ансамбль".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: Some(TITLE.into()),
            artwork: Some(ART.into()),
            added: None,
            duration: Some(18),
            media: Some(Media::File(profile.join("track-0.wav"))),
        })
        .collect();
    let page = json!({
        "reference": {"provider": "spotify", "kind": "Album", "id": ID},
        "title": TITLE,
        "artists": ["Северный ансамбль"],
        "artwork": ART,
        "url": URL,
        "tracks": tracks,
        "releases": [],
        "year": 2026,
        "single": false,
    });
    let database = Connection::open(profile.join("mediadeka.sqlite3"))?;
    database.execute(
        "INSERT INTO music_snapshot (provider, profile_id, region, slot, payload)
         VALUES ('spotify', '', ?1, ?2, ?3)",
        params![region, format!("album:{ID}"), page.to_string()],
    )?;
    for release in &SHORT_RELEASES {
        MainView::seed_artist_artwork_for_capture(profile, &release.art(), &path)?;
        let page = json!({
            "reference": {"provider": "spotify", "kind": "Album", "id": release.id},
            "title": release.title,
            "artists": ["Северный ансамбль"],
            "artwork": release.art(),
            "url": release.url(),
            "tracks": &tracks[..release.tracks],
            "releases": [],
            "year": 2026,
            "single": release.tracks == 1,
        });
        database.execute(
            "INSERT INTO music_snapshot (provider, profile_id, region, slot, payload)
             VALUES ('spotify', '', ?1, ?2, ?3)",
            params![region, format!("album:{}", release.id), page.to_string()],
        )?;
    }
    atmosphere::seed(profile)?;
    Ok(())
}

pub(super) async fn verify_short_releases(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    for (release, wall) in SHORT_RELEASES.iter().zip([12, 90]) {
        let art = release.art();
        window.update(cx, |view, window, cx| {
            view.submit_music_url_for_capture(&release.url(), window, cx);
        })?;
        wait(cx, release.title, |cx| {
            let tree = snapshot(raw, cx)?;
            let decoded = window.update(cx, |view, _, cx| {
                view.artist_artwork_decoded_for_capture(&art, cx)
            })?;
            Ok(
                (node_label(&tree, "music-object-title") == Some(release.title)
                    && decoded
                    && node_label(&tree, "music-object-play") == Some("Слушать")
                    && node_label(&tree, "music-object-releases") == Some("Другие релизы артиста"))
                .then_some(()),
            )
        })
        .await?;
        super::stage::settle_atmosphere(window, wall, cx)?;
        frame(raw, scene, output, release.name, cx)?;
        if release.tracks == 1
            && scene.width == 1920.
            && !high_contrast
            && super::super::shader_with_artwork_palette()
        {
            atmosphere::verify(window, scene, output, &art, cx).await?;
        }
    }
    Ok(())
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(URL, window, cx)
    })?;
    wait(cx, TITLE, |cx| {
        let tree = snapshot(raw, cx)?;
        let decoded = window.update(cx, |view, _, cx| {
            view.artist_artwork_decoded_for_capture(ART, cx)
        })?;
        Ok((decoded
            && node_label(&tree, "music-object-title") == Some(TITLE)
            && node_label(&tree, "music-object-play") == Some("Слушать"))
        .then_some(()))
    })
    .await?;
    let initial = frame(raw, scene, output, "album-hero", cx)?;
    require(
        node_label(&initial, HEADING) == Some(TITLE),
        "В начале доступен заголовок альбома.",
    )?;
    wheel(window, if scene.height > 560. { -170. } else { -48. }, cx)?;
    let midway = frame(raw, scene, output, "album-hero-scrolling", cx)?;
    let middle_offset = window.update(cx, |view, _, _| -view.home_scroll_offset_for_capture())?;
    require(
        middle_offset > 0. && node_label(&midway, HEADING) == Some(TITLE),
        "Заголовок альбома остаётся доступным во время перехода.",
    )?;
    wheel(window, -650., cx)?;
    let pinned = frame(raw, scene, output, "album-pinned-title", cx)?;
    let pinned_offset = window.update(cx, |view, _, _| -view.home_scroll_offset_for_capture())?;
    require(
        node_label(&pinned, HEADING) == Some(TITLE) && pinned_offset > middle_offset,
        "После прокрутки доступен закреплённый заголовок открытого альбома.",
    )?;
    wheel(window, -5000., cx)?;
    let bottom = frame(raw, scene, output, "album-bottom", cx)?;
    let bottom_offset = window.update(cx, |view, _, _| -view.home_scroll_offset_for_capture())?;
    require(
        bottom_offset > pinned_offset && node_label(&bottom, HEADING) == Some(TITLE),
        "Конец длинного альбома достигается с закреплённым заголовком и целой последней строкой.",
    )?;
    wheel(window, 6000., cx)?;
    let returned = frame(raw, scene, output, "album-hero-returned", cx)?;
    let final_offset = window.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
    require(
        node_label(&returned, HEADING) == Some(TITLE) && final_offset == 0.,
        "При возврате к началу заголовок остаётся доступным.",
    )?;
    crate::scroll_viewport::verify(
        window,
        scene,
        &output.join("album-scroll-viewport"),
        crate::scroll_viewport::Surface::Workspace,
        cx,
    )?;
    fs::write(
        output.join("album-scroll-report.json"),
        serde_json::to_vec_pretty(&json!({
            "album": TITLE,
            "scroll_offset_dip": [0., middle_offset, pinned_offset, bottom_offset, final_offset],
            "reduced_motion": scene.reduced,
            "pinned_title": true
        }))?,
    )?;
    println!(
        "{}: album hero, scroll dissolution and pinned heading, offsets {middle_offset} and {pinned_offset} DIP",
        scene.name
    );
    Ok(())
}

fn wheel(window: WindowHandle<MainView>, delta: f32, cx: &mut AsyncApp) -> gpui::Result<()> {
    let position = window.update(cx, |view, _, _| {
        view.home_scroll_bounds_for_capture().center()
    })?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    Ok(())
}
