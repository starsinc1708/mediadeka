#[path = "details/album.rs"]
mod album;
#[path = "details/artist.rs"]
mod artist;
#[path = "details/stage.rs"]
mod stage;

use std::{fs, io, path::Path};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::json;

use super::{
    Scene, frame, named, node, node_label, press, queue_candidate, require, snapshot, sound, wait,
};

pub(super) use album::{ART as ALBUM_ART, TITLE as ALBUM_TITLE, URL as ALBUM_URL};
pub(super) use artist::ARTIST_REGION;

pub(super) fn seed(profile: &Path) -> gpui::Result<()> {
    artist::seed_artists(profile)?;
    album::seed(profile, ARTIST_REGION)?;
    let files: Vec<_> = (0..8)
        .map(|index| profile.join(format!("track-{index}.wav")))
        .collect();
    let playlist = mediadeka::seed_playlist_capture(profile, &files)?;
    fs::write(profile.join("hero-playlist-id"), playlist.to_string())?;
    Ok(())
}

const PLAY: &str = "library-group-play";
const COVER: &str = "library-group-artwork";
const TITLE: &str = "music-liked-title";

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let wide = scene.width == 1920.;
    stage::verify(window, scene, output, high_contrast, cx).await?;
    for (name, rail, tab, tile, title) in [
        (
            "local-album",
            "studio-library-albums",
            "library-tab-albums",
            "library-album-0",
            "Горизонт",
        ),
        (
            "local-artist",
            "studio-library-artists",
            "library-tab-artists",
            "library-row-0",
            "Берег",
        ),
    ] {
        let destination = if wide { rail } else { tab };
        press(raw, &named(destination), cx)?;
        wait(cx, title, |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(node_label(&tree, tile)
                .is_some_and(|label| label.starts_with(title))
                .then_some(()))
        })
        .await?;
        if name == "local-album" && wide {
            album::motion::verify_card(window, scene, output, title, cx).await?;
        }
        press(raw, &named(tile), cx)?;
        if name == "local-album" {
            album::motion::verify_keyboard(window, cx)?;
        }
        let heading = wait(cx, title, |cx| {
            let tree = snapshot(raw, cx)?;
            Ok((node_label(&tree, TITLE) == Some(title)
                && node_label(&tree, PLAY) == Some("Слушать"))
            .then_some(tree))
        })
        .await?;
        require(
            node(&heading, TITLE).is_some_and(|element| element["aria"]["role"] == "Heading")
                && node(&heading, COVER).is_some_and(|element| element["aria"]["role"] == "Image"),
            "Локальный раздел должен дать заголовок и обложку с доступными ролями.",
        )?;
        let first = node_label(&heading, "library-row-0")
            .and_then(|label| label.strip_prefix("Слушать · "))
            .and_then(|label| label.split(" · ").next())
            .ok_or_else(|| io::Error::other("Нет первого трека локального раздела."))?;
        press(raw, &named(PLAY), cx)?;
        let progress = sound(window, first, cx).await?;
        frame(raw, scene, output, name, cx)?;
        press(raw, &named("workspace-back"), cx)?;
        wait(
            cx,
            "возврат к карточке медиатеки",
            |cx| {
                let tree = snapshot(raw, cx)?;
                Ok(
                    (node_label(&tree, tile).is_some_and(|label| label.starts_with(title))
                        && node(&tree, PLAY).is_none()
                        && node_label(&tree, "player-track") == Some(first))
                    .then_some(()),
                )
            },
        )
        .await?;
        fs::write(
            output.join(format!("{name}-report.json")),
            serde_json::to_vec_pretty(
                &json!({"title": title, "track": first, "playback": progress, "back_kept_track": true}),
            )?,
        )?;
        let tracks = if wide {
            "studio-library"
        } else {
            "library-tab-tracks"
        };
        press(raw, &named(tracks), cx)?;
        wait(cx, "возврат к списку треков", |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(queue_candidate(&tree).is_some().then_some(()))
        })
        .await?;
        println!(
            "{}: {title}, {first}, keyboard play and Back verified",
            scene.name
        );
    }
    album::verify(window, scene, output, cx).await?;
    artist::verify(window, scene, output, cx).await
}
