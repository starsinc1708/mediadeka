use std::{fs, io, path::Path};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::Value;

use super::{
    SCENES, Scene, ensure_same_pause, ensure_same_playback, fail, focused_id, frame, key, layout,
    load_playlist, press, record, require_node, start_guest, tree, wait,
};

const TIMED: &str =
    "[00:00.00]Метка первой строки\n[00:03.00]Метка второй строки\n[00:07.00]Последняя метка";
const PLAIN: &str = "Обычная строка первой строфы\nОтдельный второй стих";
const LOCAL_TTML: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
<p begin="0s">Первая строка из TTML</p>
<p begin="3s">Вторая строка из TTML</p>
<p begin="7s">Последняя строка из TTML</p>
</div></body></tt>"#;
const LOCAL_LRC: &str = concat!(
    "[00:00.00]<00:00.00>Первая <00:01.20>улучшенная <00:01.80>строка\n",
    "[00:03.00]<00:03.00>Следующая <00:03.70>строка\n",
    "[00:07.00]<00:07.00>Последняя <00:07.70>строка",
);
const EDITED_PLAIN: &str = "Новая первая строка\nОтдельный второй стих\nНовая последняя строка";

pub(super) fn run(output: &Path, audio: &Path, local: bool) -> gpui::Result<()> {
    let profile = tempfile::tempdir()?;
    let file = profile.path().join("Две версии текста.mp3");
    let next = profile.path().join("Следующая запись.mp3");
    fs::copy(audio, &file)?;
    fs::copy(audio, &next)?;
    fs::write(next.with_extension("lrc"), "[00:01.00]Текст другой записи")?;
    let playlist = mediadeka::seed_playlist_capture(profile.path(), &[file.clone(), next])?;
    if !local {
        MainView::seed_lyrics_versions_for_capture(profile.path(), playlist, TIMED, PLAIN)?;
    }
    // The capture process owns this profile before any workers are started.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    let output = output.to_owned();
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                let view = cx
                    .update(|cx| mediadeka::open_capture_window(cx, layout(scene)))
                    .unwrap_or_else(|error| fail(error));
                capture(
                    view,
                    scene,
                    playlist,
                    &output.join(scene.name),
                    local.then_some(file.as_path()),
                    cx,
                )
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

async fn capture(
    view: WindowHandle<MainView>,
    scene: Scene,
    playlist: i64,
    output: &Path,
    local: Option<&Path>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    if let Some(path) = local {
        let lrc = path.with_extension("lrc");
        if lrc.exists() {
            fs::remove_file(lrc)?;
        }
        fs::write(path.with_extension("ttml"), LOCAL_TTML)?;
        fs::write(path.with_extension("txt"), PLAIN)?;
    }
    let raw = AnyWindowHandle::from(view);
    start_guest(view, cx).await?;
    load_playlist(scene, view, playlist, 2, cx).await?;
    view.update(cx, |view, window, cx| {
        view.play_playlist_track_for_capture(0, cx);
        view.show_panel_for_capture("lyrics", window, cx);
    })?;
    let source = if local.is_some() {
        "Локальный файл"
    } else {
        "LRCLIB"
    };
    let mut playing = wait(view, cx, |state| {
        state["source"] == source
            && state["plain_available"] == true
            && state["timed_lines"] == 3
            && state["playing"] == true
            && state["loading"] == false
    })
    .await?;
    if let Some(path) = local {
        if !playing["word"].is_null() {
            return Err(io::Error::other("Построчный TTML неожиданно получил метки слов.").into());
        }
        frame(raw, scene, output, "before-revision", cx)?;
        record(output, "before-revision", &playing)?;
        fs::write(path.with_extension("lrc"), LOCAL_LRC)?;
        fs::write(path.with_extension("txt"), EDITED_PLAIN)?;
        view.update(cx, |view, _, cx| {
            view.seek_lyrics_playback_for_capture(1.4, cx)
        })?;
        press(raw, "lyrics-source", cx)?;
        key(raw, "down", cx)?;
        key(raw, "enter", cx)?;
        let updated = wait(view, cx, |state| {
            state["selection"] == source
                && state["plain_available"] == true
                && state["timed_lines"] == 3
                && state["word"].is_number()
                && state["playing"] == true
                && state["loading"] == false
        })
        .await?;
        ensure_same_playback(&playing, &updated)?;
        playing = updated;
    }
    frame(raw, scene, output, "synchronized", cx)?;
    record(output, "synchronized", &playing)?;

    press(raw, "player-play", cx)?;
    let paused = wait(view, cx, |state| state["playing"] == false).await?;
    press(raw, "lyrics-source", cx)?;
    let picker = frame(raw, scene, output, "version-selector", cx)?;
    require_node(&picker, "lyrics-source-plain")?;
    for _ in 0..if local.is_some() { 1 } else { 4 } {
        key(raw, "down", cx)?;
    }
    key(raw, "enter", cx)?;
    let plain = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    if plain["display_plain"] != true
        || plain["lines"] != if local.is_some() { 3 } else { 2 }
        || plain["timed_lines"] != 0
        || !plain["line"].is_null()
        || plain["loading"] != false
    {
        return Err(
            io::Error::other(format!("Не показана отдельная обычная версия: {plain}")).into(),
        );
    }
    ensure_switch_kept_playback(raw, &paused, &plain, cx)?;
    frame(raw, scene, output, "plain", cx)?;
    record(output, "plain", &plain)?;

    press(raw, "lyrics-source", cx)?;
    key(raw, "up", cx)?;
    key(raw, "enter", cx)?;
    let restored = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    if restored["display_plain"] != false
        || restored["timed_lines"] != 3
        || restored["line"].is_null()
        || restored["loading"] != false
    {
        return Err(
            io::Error::other(format!("Не восстановлена синхронная версия: {restored}")).into(),
        );
    }
    ensure_switch_kept_playback(raw, &paused, &restored, cx)?;
    frame(raw, scene, output, "synchronized-return", cx)?;
    record(output, "synchronized-return", &restored)?;
    if scene.width >= 1440. && local.is_none() {
        verify_track_page_lyrics(view, scene, output, &paused, cx).await?;
    }
    Ok(())
}

async fn verify_track_page_lyrics(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    paused: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    press(raw, "player-lyrics", cx)?;
    press(raw, "playlist-track-title-1", cx)?;
    press(raw, "music-object-lyrics", cx)?;
    let inspected = wait(view, cx, |state| {
        state["lyrics_track"] != state["current_track"] && state["timed_lines"] == 1
    })
    .await?;
    ensure_same_pause(paused, &inspected)?;
    if inspected["follows_playback"] != false
        || inspected["seekable"] != false
        || !inspected["line"].is_null()
        || inspected["plain_available"] != false
    {
        return Err(io::Error::other(format!(
            "Панель использует позицию другого трека: {inspected}"
        ))
        .into());
    }
    frame(raw, scene, output, "inspected-track", cx)?;
    record(output, "inspected-track", &inspected)?;

    press(raw, "player-lyrics", cx)?;
    press(raw, "player-lyrics", cx)?;
    let current = wait(view, cx, |state| {
        state["lyrics_track"] == state["current_track"]
            && state["follows_playback"] == true
            && state["timed_lines"] == 3
            && state["seekable"] == true
    })
    .await?;
    ensure_same_pause(paused, &current)?;
    frame(raw, scene, output, "return-to-player", cx)?;
    record(output, "return-to-player", &current)
}

pub(super) fn ensure_switch_kept_playback(
    raw: AnyWindowHandle,
    before: &Value,
    after: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    ensure_same_playback(before, after)?;
    if after["playing"] != false || before["player_position"] != after["player_position"] {
        return Err(io::Error::other("Смена версии нарушила паузу или позицию.").into());
    }
    if focused_id(&tree(raw, cx)?) != Some("Name(\"lyrics-source\")") {
        return Err(io::Error::other("Фокус не вернулся к селектору версий.").into());
    }
    Ok(())
}
