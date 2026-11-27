use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, MouseButton, WindowHandle};
use mediadeka::{CaptureLayout, MainView, providers::ProviderId};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::Value;
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use super::{Scene, draw, fail, home::click, native_dpi_change};

const UIA_ROOT_OBJECT_ID: isize = -25;
const SCENES: [Scene; 2] = [
    Scene {
        name: "1500x850",
        width: 1500.,
        height: 850.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "800x400",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: true,
    },
];

pub(super) fn run(args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let arguments = args.collect::<Vec<_>>();
    let [output, audio] = arguments.as_slice() else {
        return Err(io::Error::other("--sound-stage5 <каталог кадров> <аудиофайл>").into());
    };
    let output = PathBuf::from(output);
    fs::create_dir_all(&output)?;
    let audio = PathBuf::from(audio).canonicalize()?;
    let profile = tempfile::tempdir()?;
    let playlist =
        mediadeka::seed_playlist_capture(profile.path(), &[audio.clone(), audio.clone()])?;
    // This capture process owns the temporary profile; GPUI has not started yet.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                if let Err(error) = capture(scene, playlist, &audio, &output, cx).await {
                    fail(error);
                }
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

fn layout(scene: Scene) -> CaptureLayout {
    CaptureLayout {
        width: scene.width,
        height: scene.height,
        reduced_motion: scene.reduced,
    }
}

async fn capture(
    scene: Scene,
    playlist: i64,
    audio: &Path,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(playlist, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(12);
    while view.update(cx, |view, _, _| view.playlist_status_for_capture())? != Some(Ok(2)) {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Тестовый плейлист не открылся.").into());
        }
        timer.timer(Duration::from_millis(35)).await;
    }
    view.update(cx, |view, window, cx| {
        view.select_playlist_track_for_capture(0, window, cx);
        view.play_playlist_track_for_capture(0, cx);
        view.show_panel_for_capture("track", window, cx);
    })?;
    let state = loop {
        if let Some(state) = view.update(cx, |view, _, cx| view.sound_state_for_capture(cx))?
            && state["position"].as_f64().is_some_and(|value| value > 0.1)
        {
            break state;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Движок не подтвердил звук и формат записи.").into());
        }
        timer.timer(Duration::from_millis(35)).await;
    };
    let label = format!(
        "{} · {}",
        state["source"].as_str().unwrap_or_default(),
        state["quality"].as_str().unwrap_or_default()
    );
    fs::write(
        output.join("audio.json"),
        serde_json::to_vec_pretty(&state)?,
    )?;
    draw(raw, scene, Some(&output.join("playing.png")), cx)?;
    let tree = accessibility(raw, &output.join("playing-a11y.json"), cx)?;
    if node_label(&tree, "player-sound-details") != Some(label.as_str())
        || node_label(&tree, "track-panel-sound-details") != Some(label.as_str())
    {
        return Err(
            io::Error::other(format!("Интерфейс не показал данные движка: {label}")).into(),
        );
    }

    if scene.name == SCENES[0].name {
        let diagnostics = view.update(cx, |view, window, cx| {
            view.open_sound_diagnostics_for_capture(window, cx)
        })??;
        capture_diagnostics(diagnostics, &output, &state, &label, cx).await?;
    }
    view.update(cx, |view, window, cx| {
        view.select_playlist_track_for_capture(1, window, cx)
    })?;
    draw(raw, scene, Some(&output.join("other-track.png")), cx)?;
    let tree = accessibility(raw, &output.join("other-track-a11y.json"), cx)?;
    if node_label(&tree, "player-sound-details") != Some(label.as_str())
        || node_label(&tree, "track-panel-sound-details").is_some()
    {
        return Err(
            io::Error::other("Качество звучащего трека показано у другого добавления.").into(),
        );
    }

    view.update(cx, |view, window, cx| {
        view.close_right_panel_for_capture(window, cx);
        view.sample_library_for_capture(audio, window, cx);
        view.set_library_filters_for_capture(cx);
        view.reveal_album_rows_for_capture(true, cx);
    })?;
    draw(raw, scene, Some(&output.join("downloaded.png")), cx)?;
    let tree = accessibility(raw, &output.join("downloaded-a11y.json"), cx)?;
    let library = view.update(cx, |view, _, _| view.liked_audit_for_capture())?;
    if library["tracks"] != 1
        || library["downloaded"] != 1
        || library["only_downloaded"] != true
        || !node_label(&tree, "library-row-0").is_some_and(|label| label.contains("Скачано"))
    {
        return Err(io::Error::other("В отборе скачанных не виден знак строки.").into());
    }
    let duration = state["duration"]
        .as_f64()
        .ok_or_else(|| io::Error::other("У тестового файла нет длительности."))?
        .round() as u32;
    capture_cached_row(view, scene, audio, duration, &output, cx).await?;
    capture_settings(view, scene, &output, cx).await?;
    println!("{}: {label}", scene.name);
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn capture_cached_row(
    view: WindowHandle<MainView>,
    scene: Scene,
    audio: &Path,
    duration: u32,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, _, cx| {
        view.sample_cached_track_for_capture(audio, duration, cx)
    })??;
    view.update(cx, |view, _, cx| {
        view.reveal_album_rows_for_capture(true, cx)
    })?;
    draw(raw, scene, None, cx)?;
    let row = view
        .update(cx, |view, _, _| view.first_library_row_for_capture())?
        .ok_or_else(|| io::Error::other("Скачанный трек отсутствует в списке."))?;
    let path = output.join("cached-a11y.json");
    let tree = accessibility(raw, &path, cx)?;
    if !node_label(&tree, "library-row-0")
        .is_some_and(|label| label.contains("Огни") && label.contains("Скачано"))
    {
        return Err(io::Error::other("В отборе не виден скачанный трек Spotify.").into());
    }
    click(raw, MouseButton::Left, row, cx)?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(state) = view.update(cx, |view, _, cx| view.sound_state_for_capture(cx))?
            && state["source"] == "Кэш профиля"
            && state["position"].as_f64().is_some_and(|value| value > 0.1)
        {
            fs::write(
                output.join("cached-audio.json"),
                serde_json::to_vec_pretty(&state)?,
            )?;
            break;
        }
        if Instant::now() >= deadline {
            return Err(
                io::Error::other("Выбор скачанной строки не запустил кэш напрямую.").into(),
            );
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    draw(raw, scene, Some(&output.join("cached.png")), cx)?;
    Ok(())
}

async fn capture_diagnostics(
    raw: AnyWindowHandle,
    output: &Path,
    state: &Value,
    label: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let scene = Scene {
        name: "diagnostics",
        width: 770.,
        height: 630.,
        ..SCENES[0]
    };
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    let reason = state["reason"]
        .as_str()
        .ok_or_else(|| io::Error::other("Движок не записал причину выбора источника."))?;
    let time = state["time_to_sound_ms"]
        .as_u64()
        .ok_or_else(|| io::Error::other("Движок не записал время до старта звука."))?;
    let expected = format!("Аудио · {label} · До звука {time} мс · {reason}");
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(3);
    let tree_path = output.join("diagnostics-a11y.json");
    loop {
        draw(raw, scene, None, cx)?;
        if node_label(&accessibility(raw, &tree_path, cx)?, "diagnostics-sound")
            == Some(expected.as_str())
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(
                io::Error::other("Диагностика не показала решение и время до звука.").into(),
            );
        }
        timer.timer(Duration::from_millis(50)).await;
    }
    draw(raw, scene, Some(&output.join("diagnostics.png")), cx)?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn capture_settings(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    let first = scene.name == SCENES[0].name;
    let changed = if first {
        ProviderId::SoundCloud
    } else {
        ProviderId::Jellyfin
    };
    for (state, enabled) in [("settings", true), ("settings-switched", false)] {
        if !enabled {
            view.update(cx, |view, _, cx| {
                view.toggle_sound_download_for_capture(changed, cx)
            })?;
        }
        let jellyfin = first || enabled;
        let soundcloud = first && enabled;
        let timer = cx.background_executor().clone();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let current = view.update(cx, |view, _, _| {
                (
                    view.sound_download_enabled_for_capture(ProviderId::Jellyfin),
                    view.sound_download_enabled_for_capture(ProviderId::SoundCloud),
                )
            })?;
            if current == (Some(jellyfin), Some(soundcloud)) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "Параметры загрузки не применились: {current:?}"
                ))
                .into());
            }
            timer.timer(Duration::from_millis(40)).await;
        }
        draw(raw, scene, None, cx)?;
        view.update(cx, |view, _, cx| {
            view.reveal_sound_downloads_for_capture(cx)
        })?;
        draw(raw, scene, Some(&output.join(format!("{state}.png"))), cx)?;
        let tree = accessibility(raw, &output.join(format!("{state}-a11y.json")), cx)?;
        for (provider, enabled) in [
            (ProviderId::Jellyfin, jellyfin),
            (ProviderId::SoundCloud, soundcloud),
        ] {
            let id = format!("settings-audio-download-{}", provider.as_str());
            let expected = if enabled { "True" } else { "False" };
            if !node(&tree, &id).is_some_and(|node| node["aria"]["toggled"] == expected) {
                return Err(io::Error::other(format!(
                    "Переключатель {} не показал состояние {expected}.",
                    provider.display_name()
                ))
                .into());
            }
        }
    }
    Ok(())
}

pub(super) fn activate_accessibility(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    let hwnd = raw.update(cx, |_, window, _| -> gpui::Result<_> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("Ожидалось окно Win32.").into());
        };
        Ok(handle.hwnd.get())
    })??;
    // Fetch the root provider of the hidden GPUI window without synthesizing input.
    unsafe { SendMessageW(hwnd as _, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID) };
    Ok(())
}

pub(super) fn accessibility(
    raw: AnyWindowHandle,
    path: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let tree = raw
        .update(cx, |_, window, _| window.debug_a11y_tree_json())?
        .ok_or_else(|| io::Error::other("GPUI не построил дерево доступности."))?;
    fs::write(path, &tree)?;
    Ok(serde_json::from_str(&tree)?)
}

pub(super) fn node_label<'a>(tree: &'a Value, id: &str) -> Option<&'a str> {
    node(tree, id)?["aria"]["label"].as_str()
}

pub(super) fn node<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    // GPUI's debug dump formats a string ElementId using its enum variant.
    let id = format!("Name({id:?})");
    tree["nodes"]
        .as_object()?
        .values()
        .find(|node| node["element_id"] == id)
}
