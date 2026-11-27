use std::{collections::BTreeSet, fs, io, path::Path};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{
    Scene,
    mix::{
        accessibility_tree, activate_control, check_button, check_queue_rows, frame, press_enter,
        scroll_queue_to_top, wait_until,
    },
};
use crate::sound::{activate_accessibility, node};

const AUTOPLAY: &str = "queue-autoplay";
const TRACK_MENU: &str = "music-object-track-menu";
const TRACK_RADIO: &str = "track-radio-action";
const ARTIST_RADIO: &str = "music-object-artist-radio";
const STOP_RADIO: &str = "queue-radio-stop";
const CLOSE_PANEL: &str = "right-panel-close";

struct Example {
    evidence: &'static str,
    url: &'static str,
    title: &'static str,
    playlist: &'static str,
    track_id: Option<&'static str>,
}

const EXAMPLES: [Example; 3] = [
    Example {
        evidence: "first-track",
        url: "https://open.spotify.com/track/7GhIk7Il098yCjg4BQjzvb",
        title: "Never Gonna Give You Up",
        playlist: "Never Gonna Give You Up Radio",
        track_id: Some("7GhIk7Il098yCjg4BQjzvb"),
    },
    Example {
        evidence: "second-track",
        url: "https://open.spotify.com/track/3AJwUDP919kvQ9QcozQPxg",
        title: "Yellow",
        playlist: "Yellow Radio",
        track_id: Some("3AJwUDP919kvQ9QcozQPxg"),
    },
    Example {
        evidence: "artist",
        url: "https://open.spotify.com/artist/4gzpq5DPGxSnKTe4SA8HAU",
        title: "Coldplay",
        playlist: "Coldplay Radio",
        track_id: None,
    },
];

pub(super) async fn capture(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.show_saved_home_for_mix_capture(window, cx)
    })??;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    if node(&accessibility_tree(raw, cx)?, CLOSE_PANEL).is_some() {
        activate_control(raw, CLOSE_PANEL, cx)?;
    }

    let mut recommendations = Vec::new();
    for (index, example) in EXAMPLES.iter().enumerate() {
        open_page(view, raw, example, cx).await?;
        if index == 0 {
            capture_autoplay(raw, scene, &output, cx).await?;
        }
        if example.track_id.is_some() {
            activate_control(raw, TRACK_MENU, cx)?;
            let menu = frame(
                raw,
                scene,
                &output,
                &format!("{}-menu", example.evidence),
                cx,
            )?;
            if !node(&menu, TRACK_RADIO).is_some_and(|item| {
                item["aria"]["role"] == "MenuItem" && item["aria"]["label"] == "Радио по треку"
            }) {
                return Err(io::Error::other("В меню трека нет доступного действия радио.").into());
            }
            press_enter(raw, cx)?;
        } else {
            let tree = frame(raw, scene, &output, "artist-action", cx)?;
            check_button(&tree, ARTIST_RADIO, "Радио по исполнителю")?;
            activate_control(raw, ARTIST_RADIO, cx)?;
        }
        let list = capture_queue(view, raw, scene, &output, example, cx).await?;
        println!(
            "GPUI {}: {} · {} рекомендаций.",
            scene.name,
            example.playlist,
            list.len()
        );
        recommendations.push(list);
        activate_control(raw, STOP_RADIO, cx)?;
        wait_until(raw, cx, "остановка радио", |tree, _| {
            node(tree, STOP_RADIO).is_none().then_some(())
        })
        .await?;
        activate_control(raw, CLOSE_PANEL, cx)?;
    }
    if recommendations[0] == recommendations[1] {
        return Err(io::Error::other("Оба трека получили одинаковый состав радио.").into());
    }
    Ok(())
}

async fn open_page(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    example: &Example,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(example.url, window, cx)
    })?;
    let (kind, control) = if example.track_id.is_some() {
        ("track", TRACK_MENU)
    } else {
        ("artist", ARTIST_RADIO)
    };
    wait_until(raw, cx, example.title, |tree, cx| {
        let details = view
            .update(cx, |view, _, _| view.music_semantics_for_capture())
            .ok()?;
        (node(tree, control).is_some()
            && details["details"]["kind"] == kind
            && details["details"]["title"]
                .as_str()
                .is_some_and(|title| title.starts_with(example.title)))
        .then_some(())
    })
    .await
}

async fn capture_autoplay(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    activate_control(raw, "music-object-play", cx)?;
    activate_control(raw, "player-queue", cx)?;
    let off = frame(raw, scene, output, "autoplay-off", cx)?;
    check_autoplay(&off, "False")?;
    activate_control(raw, AUTOPLAY, cx)?;
    let on = frame(raw, scene, output, "autoplay-on", cx)?;
    check_autoplay(&on, "True")?;
    activate_control(raw, AUTOPLAY, cx)?;
    wait_until(
        raw,
        cx,
        "выключение автопродолжения",
        |tree, _| check_autoplay(tree, "False").ok(),
    )
    .await?;
    activate_control(raw, CLOSE_PANEL, cx)?;
    wait_until(raw, cx, "закрытие очереди", |tree, _| {
        node(tree, CLOSE_PANEL).is_none().then_some(())
    })
    .await
}

fn check_autoplay(tree: &Value, toggled: &str) -> gpui::Result<()> {
    if !node(tree, AUTOPLAY).is_some_and(|control| {
        let aria = &control["aria"];
        aria["role"] == "Switch" && aria["label"] == "Автопродолжение" && aria["toggled"] == toggled
    }) {
        return Err(io::Error::other(format!(
            "Автопродолжение не получило доступное состояние {toggled}."
        ))
        .into());
    }
    Ok(())
}

async fn capture_queue(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    example: &Example,
    cx: &mut AsyncApp,
) -> gpui::Result<BTreeSet<(String, String)>> {
    let seed_rows = usize::from(example.track_id.is_some());
    let model = wait_until(raw, cx, example.playlist, |tree, cx| {
        node(tree, STOP_RADIO)?;
        let model = view
            .update(cx, |view, _, cx| view.home_mix_queue_for_capture(cx))
            .ok()??;
        model["queue"]
            .as_array()?
            .iter()
            .skip(seed_rows)
            .any(|row| has_station_origin(row, example.playlist))
            .then_some(model)
    })
    .await?;
    let rows = model["queue"]
        .as_array()
        .ok_or_else(|| io::Error::other("Нет данных очереди радио."))?;
    if model["current_index"] != 0
        || example.track_id.is_some_and(|id| {
            rows[0]["catalog_id"] != id
                || rows[0]["origins"]
                    .as_array()
                    .is_none_or(|origins| !origins.is_empty())
        })
        || rows[seed_rows..]
            .iter()
            .any(|row| row["origins"].as_array().is_none_or(Vec::is_empty))
    {
        return Err(io::Error::other("В очереди радио неверный старт или происхождение.").into());
    }
    scroll_queue_to_top(raw, scene, cx)?;
    let name = format!("{}-queue", example.evidence);
    let tree = frame(raw, scene, output, &name, cx)?;
    let labels = check_queue_rows(raw, scene, rows, tree, cx)?;
    fs::write(
        output.join(format!("{name}-provenance.json")),
        serde_json::to_vec_pretty(&json!({"model": model, "accessible_labels": labels}))?,
    )?;
    scroll_queue_to_top(raw, scene, cx)?;
    rows[seed_rows..].iter().map(recording_key).collect()
}

fn has_station_origin(row: &Value, playlist: &str) -> bool {
    row["origins"].as_array().is_some_and(|origins| {
        origins
            .iter()
            .any(|origin| origin["provider"] == "Spotify" && origin["playlist"] == playlist)
    })
}

fn recording_key(row: &Value) -> gpui::Result<(String, String)> {
    let error = || io::Error::other("Рекомендация не содержит идентификатор записи.");
    let recording = &row["recording"];
    let provider = recording["provider"].as_str().ok_or_else(error)?;
    let id = recording["id"].as_str().ok_or_else(error)?;
    Ok((provider.to_owned(), id.to_owned()))
}
