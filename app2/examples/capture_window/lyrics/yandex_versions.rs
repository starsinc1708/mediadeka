use std::{
    ffi::OsString,
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;

use super::{
    SCENES, Scene, activate_accessibility, fail, frame, key, layout, native_dpi_change, press,
    record, require_node, tree, versions::ensure_switch_kept_playback, wait,
};

const TRACK: &str = "https://music.yandex.ru/album/1612097/track/10741527";
const TRACK_ID: &str = "10741527:1612097";
const SOURCE: &str = "Яндекс Музыка";

pub(super) fn run(arguments: &[OsString]) -> gpui::Result<()> {
    let [output, profile] = arguments else {
        return Err(io::Error::other(
            "--lyrics-stage6 --yandex-versions <каталог кадров> <изолированный профиль с Яндекс Музыкой>",
        ).into());
    };
    let output = Path::new(output).to_owned();
    let profile = Path::new(profile);
    if !profile.join("mediadeka.sqlite3").is_file() {
        return Err(io::Error::other("Изолированный профиль не содержит базу данных.").into());
    }
    // GPUI workers have not started. This process owns the supplied profile copy.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile) };
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            for scene in SCENES {
                let view = cx
                    .update(|cx| mediadeka::open_capture_window(cx, layout(scene)))
                    .unwrap_or_else(|error| fail(error));
                verify(view, scene, &output.join(scene.name), cx)
                    .await
                    .unwrap_or_else(|error| fail(error));
                let _ = view.update(cx, |_, window, _| window.remove_window());
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.show_saved_home_for_mix_capture(window, cx)
    })??;
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(TRACK, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(16);
    while require_node(&tree(raw, cx)?, "music-object-play").is_err() {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Не открылась запись Яндекс Музыки.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    press(raw, "music-object-play", cx)?;
    press(raw, "player-lyrics", cx)?;
    wait(view, cx, |state| {
        state["source"].is_string() && state["playing"] == true
    })
    .await?;
    press(raw, "lyrics-source", cx)?;
    key(raw, "down", cx)?;
    key(raw, "down", cx)?;
    key(raw, "enter", cx)?;
    wait(view, cx, |state| {
        state["plain_available"] == true
            && state["source"] == SOURCE
            && state["selection"] == SOURCE
            && state["lyrics_track"] == TRACK_ID
            && state["current_track"] == TRACK_ID
            && state["timed_lines"] == 46
            && state["playing"] == true
            && state["player_position"]
                .as_f64()
                .is_some_and(|position| position > 0.1)
    })
    .await?;
    view.update(cx, |view, _, cx| {
        view.seek_lyrics_playback_for_capture(13., cx)
    })?;
    let synchronized = wait(view, cx, |state| {
        state["line"].is_number()
            && state["player_position"]
                .as_f64()
                .is_some_and(|position| (12.9..=15.).contains(&position))
    })
    .await?;
    frame(raw, scene, output, "synchronized", cx)?;
    record(output, "synchronized", &synchronized)?;

    press(raw, "player-play", cx)?;
    let paused = wait(view, cx, |state| state["playing"] == false).await?;
    press(raw, "lyrics-source", cx)?;
    let picker = frame(raw, scene, output, "versions", cx)?;
    require_node(&picker, "lyrics-source-plain")?;
    key(raw, "down", cx)?;
    key(raw, "enter", cx)?;
    let plain = wait(view, cx, |state| {
        state["source"] == SOURCE
            && state["display_plain"] == true
            && state["timed_lines"] == 0
            && state["lines"] == 49
            && state["line"].is_null()
    })
    .await?;
    ensure_switch_kept_playback(raw, &paused, &plain, cx)?;
    frame(raw, scene, output, "plain", cx)?;
    record(output, "plain", &plain)?;

    press(raw, "lyrics-source", cx)?;
    key(raw, "up", cx)?;
    key(raw, "enter", cx)?;
    let restored = wait(view, cx, |state| {
        state["source"] == SOURCE && state["timed_lines"] == 46 && state["line"].is_number()
    })
    .await?;
    ensure_switch_kept_playback(raw, &paused, &restored, cx)?;
    frame(raw, scene, output, "synchronized-return", cx)?;
    record(output, "synchronized-return", &restored)
}
