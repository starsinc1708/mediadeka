use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, MouseButton, WindowHandle,
    point, px,
};
use mediadeka::MainView;

use super::{Scene, activate_control, click, draw, mix::accessibility_tree};
use crate::sound::{activate_accessibility, node};

const QUEUE_TRIGGER: &str = "home-for-you-queue";

pub(super) async fn capture(
    id: i64,
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let fluid = std::env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3");
    if fluid {
        activate_accessibility(raw, cx)?;
    }
    view.update(cx, |view, window, cx| {
        view.open_home_listening_fixture(id, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(15);
    while view.update(cx, |view, _, _| view.playlist_status_for_capture())? != Some(Ok(2)) {
        if Instant::now() > deadline {
            return Err(io::Error::other("Не открылся локальный плейлист из двух дорожек.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    view.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(0, cx)
    })?;
    loop {
        let (_, position, playing, _, _, count) =
            view.update(cx, |view, _, cx| view.home_listening_fixture_state(id, cx))?;
        if playing && position > 0.2 && count == 2 {
            break;
        }
        if Instant::now() > deadline {
            return Err(
                io::Error::other("Состояние реального воспроизведения не подтвердилось.").into(),
            );
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    view.update(cx, |view, _, cx| view.seek_home_listening_fixture(cx))?;
    loop {
        if view.update(cx, |view, _, cx| {
            view.home_listening_fixture_state(id, cx).1
        })? > 2.0
        {
            break;
        }
        if Instant::now() > deadline {
            return Err(io::Error::other("Плеер не подтвердил перемотку тестовой записи.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    view.update(cx, |view, window, cx| {
        view.pause_home_listening_fixture(cx);
        view.return_home_from_listening_fixture(window, cx);
    })?;
    let (saved, prior_recent) = loop {
        let (saved, position, playing, fraction, recent, _) =
            view.update(cx, |view, _, cx| view.home_listening_fixture_state(id, cx))?;
        if let Some(saved) = saved
            && !playing
            && saved >= 1_300
            && (saved as f64 / 1_000. - position).abs() < 0.3
            && fraction > 0.15
            && recent > 0
        {
            break (saved, recent);
        }
        if Instant::now() > deadline {
            return Err(io::Error::other("Прогресс паузы не появился в истории главной.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    };
    draw(
        raw,
        scene,
        Some(&output.join(format!("home-{}-listening.png", scene.name))),
        cx,
    )?;
    // Let an ordinary player position update race with the SQLite read. The
    // resume must return to the stored position on the same track.
    view.update(cx, |view, _, cx| view.pause_home_listening_fixture(cx))?;
    loop {
        if view.update(cx, |view, _, cx| {
            view.home_listening_fixture_state(id, cx).2
        })? {
            break;
        }
        if Instant::now() > deadline {
            return Err(io::Error::other("Плеер не снялся с паузы.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    let compact = scene.width == 800.;
    let card = if compact { (190., 211.) } else { (380., 223.) };
    click(raw, MouseButton::Left, point(px(card.0), px(card.1)), cx)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let (offset, fraction, recent) = loop {
        let (_, position, playing, fraction, recent, count) =
            view.update(cx, |view, _, cx| view.home_listening_fixture_state(id, cx))?;
        if playing
            && (saved as f64 / 1_000. - position).abs() < 0.5
            && count == 2
            && recent > prior_recent
            && fraction > 0.15
        {
            break (position, fraction, recent);
        }
        if Instant::now() > deadline {
            return Err(
                io::Error::other("Главная не восстановила сохранённые позицию и очередь.").into(),
            );
        }
        timer.timer(Duration::from_millis(40)).await;
    };
    fs::write(
        output.join(format!("home-{}-listening.json", scene.name)),
        serde_json::to_vec_pretty(&serde_json::json!({
            "saved_ms": saved, "resumed_seconds": offset, "progress_fraction": fraction,
            "recent_history_records": recent, "queue_length": 2,
        }))?,
    )?;
    if compact {
        let origin = view.update(cx, |view, _, _| view.home_for_you_origin_for_capture())?;
        view.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture((origin - 6.).max(0.), cx)
        })?;
        draw(
            raw,
            scene,
            Some(&output.join(format!("home-{}-for-you.png", scene.name))),
            cx,
        )?;
        if fluid {
            let tree = accessibility_tree(raw, cx)?;
            if node(&tree, QUEUE_TRIGGER).is_none_or(|button| {
                button["aria"]["label"] != "Очередь · 2" || button["aria"]["expanded"] != false
            }) {
                return Err(
                    io::Error::other("Состояние кнопки «Очередь» на главной недоступно.").into(),
                );
            }
            activate_control(raw, QUEUE_TRIGGER, cx)?;
        } else {
            click(raw, MouseButton::Left, point(px(724.), px(74.)), cx)?;
        }
        if !view.update(cx, |view, _, _| view.home_fixture_queue_open())? {
            return Err(
                io::Error::other("Кнопка «Очередь» в блоке «Для вас» не открыла панель.").into(),
            );
        }
        draw(
            raw,
            scene,
            Some(&output.join(format!("home-{}-for-you-queue.png", scene.name))),
            cx,
        )?;
        if fluid {
            for (tab, marker) in [
                ("right-panel-track", "track"),
                ("right-panel-lyrics", "lyrics"),
            ] {
                activate_control(raw, tab, cx)?;
                draw(
                    raw,
                    scene,
                    Some(&output.join(format!("home-{}-panel-{marker}.png", scene.name))),
                    cx,
                )?;
                if node(&accessibility_tree(raw, cx)?, tab)
                    .is_none_or(|node| node["aria"]["selected"] != true)
                {
                    return Err(io::Error::other(format!(
                        "Правая панель не выбрала вкладку {tab}."
                    ))
                    .into());
                }
            }
            let escape = Keystroke::parse("escape")?;
            raw.update(cx, |_, window, cx| {
                window.dispatch_keystroke(escape.clone(), cx);
                window.dispatch_event(KeyUpEvent { keystroke: escape }.to_platform_input(), cx);
                window.draw(cx).clear(cx);
            })?;
            let returned = accessibility_tree(raw, cx)?;
            fs::write(
                output.join(format!("home-{}-queue-return.json", scene.name)),
                serde_json::to_vec_pretty(&returned)?,
            )?;
            let focus = returned["gpui_focus"]
                .as_str()
                .and_then(|id| returned["nodes"][id]["element_id"].as_str());
            let expected = format!("Name({QUEUE_TRIGGER:?})");
            if node(&returned, "right-panel").is_some() || focus != Some(expected.as_str()) {
                return Err(io::Error::other(format!(
                    "Escape не вернул фокус кнопке «Очередь» на главной: {focus:?}."
                ))
                .into());
            }
        }
    }
    Ok(())
}
