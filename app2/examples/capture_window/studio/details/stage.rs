//! Stills of the editorial stage shared by album, artist and playlist pages.
//!
//! These frames are captured before the older interaction checks so the large
//! hero can be reviewed even when a legacy motion assertion no longer applies.

use std::{fs, io, path::Path, time::Duration};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use mediadeka::MainView;
use serde_json::json;

use super::super::{Scene, frame, node, node_label, queue_candidate, require, snapshot, wait};

const ALBUM_ART: &str = "https://i.scdn.co/image/fluid-scroll-album";
const ARTIST_URL: &str = "https://open.spotify.com/artist/ArtistPortraitLarge001";
const ARTIST_ART: &str = "https://i.scdn.co/image/artist-hero-large";
const ARTIST_TITLE: &str = "Северный ансамбль";
const PLAYLIST_ID_FILE: &str = "hero-playlist-id";
const PLAYLIST_TITLE: &str = "playlist-title";
const HEADING: &str = "music-object-title";

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let mut report = serde_json::Map::new();

    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(super::album::URL, window, cx)
    })?;
    wait(cx, "альбомная сцена", |cx| {
        let tree = snapshot(raw, cx)?;
        let decoded = window.update(cx, |view, _, cx| {
            view.artist_artwork_decoded_for_capture(ALBUM_ART, cx)
        })?;
        Ok((decoded
            && node_label(&tree, "music-object-title") == Some(super::album::TITLE)
            && node_label(&tree, "music-object-play") == Some("Слушать"))
        .then_some(()))
    })
    .await?;
    settle(cx).await;
    settle_atmosphere(window, 4, cx)?;
    frame(raw, scene, output, "stage-album", cx)?;
    wheel(window, -650., cx)?;
    let pinned = wait(
        cx,
        "закреплённый заголовок альбома",
        |cx| {
            let tree = snapshot(raw, cx)?;
            let offset = window.update(cx, |view, _, _| -view.home_scroll_offset_for_capture())?;
            Ok(
                (offset > 0. && node_label(&tree, HEADING) == Some(super::album::TITLE))
                    .then_some(tree),
            )
        },
    )
    .await?;
    require(
        node(&pinned, HEADING).is_some(),
        "Крупный Hero должен оставлять закреплённый заголовок.",
    )?;
    frame(raw, scene, output, "stage-album-pinned", cx)?;
    report.insert("album_pinned".into(), true.into());

    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(ARTIST_URL, window, cx)
    })?;
    wait(cx, "артистическая сцена", |cx| {
        let tree = snapshot(raw, cx)?;
        let decoded = window.update(cx, |view, _, cx| {
            view.artist_artwork_decoded_for_capture(ARTIST_ART, cx)
        })?;
        Ok(
            (decoded && node_label(&tree, "music-object-title") == Some(ARTIST_TITLE))
                .then_some(()),
        )
    })
    .await?;
    settle(cx).await;
    settle_atmosphere(window, 8, cx)?;
    frame(raw, scene, output, "stage-artist", cx)?;
    super::album::verify_short_releases(window, scene, output, high_contrast, cx).await?;

    let profile = std::env::var("MEDIADEKA_PROFILE_DIR")
        .map_err(|error| io::Error::other(format!("Нет профиля сцены: {error}")))?;
    let id = fs::read_to_string(Path::new(&profile).join(PLAYLIST_ID_FILE))
        .map_err(|error| io::Error::other(format!("Нет идентификатора плейлиста: {error}")))?
        .trim()
        .parse::<i64>()
        .map_err(|error| {
            io::Error::other(format!("Некорректный идентификатор плейлиста: {error}"))
        })?;
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(id, window, cx)
    })?;
    let count = wait(cx, "сцена плейлиста", |cx| {
        snapshot(raw, cx)?;
        Ok(
            match window.update(cx, |view, _, _| view.playlist_status_for_capture())? {
                Some(Ok(count)) if count > 0 => Some(count),
                Some(Err(error)) => return Err(io::Error::other(error).into()),
                _ => None,
            },
        )
    })
    .await?;
    settle(cx).await;
    settle_atmosphere(window, 100, cx)?;
    let playlist = frame(raw, scene, output, "stage-playlist", cx)?;
    require(
        node(&playlist, PLAYLIST_TITLE).is_some(),
        "Заголовок плейлиста должен оставаться доступным.",
    )?;
    report.insert("playlist_tracks".into(), count.into());
    fs::write(
        output.join("stage-report.json"),
        serde_json::to_vec_pretty(&json!({
            "album": super::album::TITLE,
            "artist": ARTIST_TITLE,
            "playlist_tracks": count,
            "width": scene.width,
            "height": scene.height,
            "dpi": scene.dpi,
            "high_contrast": high_contrast,
            "stage": report,
        }))?,
    )?;

    window.update(cx, |view, window, cx| {
        view.resume_fluid_glass_for_capture(cx);
        view.show_music_collection_for_capture(false, window, cx)
    })?;
    wait(
        cx,
        "возврат к медиатеке после сцены",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(queue_candidate(&tree).is_some().then_some(()))
        },
    )
    .await?;
    println!("{}: album, artist and playlist stage stills", scene.name);
    Ok(())
}

pub(super) fn settle_atmosphere(
    window: WindowHandle<MainView>,
    seconds: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    for offset in [0, 2] {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_palette_for_capture(
                Duration::from_secs(seconds + offset),
                Duration::from_secs(4),
                cx,
            );
        })?;
        snapshot(window.into(), cx)?;
    }
    Ok(())
}

async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(420))
        .await;
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
