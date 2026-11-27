use std::{path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, Keystroke, WindowHandle};
use mediadeka::MainView;
use serde_json::Value;

use super::{
    Scene, dispatch, focus, focused, frame, key, named, node, node_label, press, queue_candidate,
    require, snapshot, wait,
};

struct Section {
    rail: &'static str,
    tab: &'static str,
    title: &'static str,
    count: &'static str,
}

const TRACKS: Section = Section {
    rail: "studio-library",
    tab: "library-tab-tracks",
    title: "Треки",
    count: "8 треков",
};
const ALBUMS: Section = Section {
    rail: "studio-library-albums",
    tab: "library-tab-albums",
    title: "Альбомы",
    count: "8 альбомов",
};
const ARTISTS: Section = Section {
    rail: "studio-library-artists",
    tab: "library-tab-artists",
    title: "Исполнители",
    count: "6 исполнителей",
};
const PLAYLISTS: Section = Section {
    rail: "home-playlists",
    tab: "library-tab-playlists",
    title: "Плейлисты",
    count: "1 плейлист",
};

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    initial: &Value,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let expanded = scene.width == 1920.;
    let animated = !scene.reduced && !high_contrast;
    require(
        node_label(initial, "home-search") == Some("Поиск музыки")
            && node_label(initial, "home-catalog-search") == Some("Начать поиск музыки"),
        "В верхней панели должны быть именованные поле и кнопка поиска.",
    )?;
    check_selection(initial, &TRACKS, expanded)?;
    let (tracks_top, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    require(tracks_top.is_some(), "У треков должен быть видимый маркер.")?;
    let mut previous_top = tracks_top;

    for (section, capture) in [
        (&ALBUMS, Some("navigation-albums")),
        (&TRACKS, None),
        (&ARTISTS, Some("navigation-artists")),
        (&PLAYLISTS, Some("navigation-playlists")),
    ] {
        let control = if expanded || section.rail == PLAYLISTS.rail {
            section.rail
        } else {
            section.tab
        };
        press(raw, &named(control), cx)?;
        let (visible, motion_start) =
            window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
        require(
            motion_start == if animated { previous_top } else { None },
            &format!(
                "Маркер вкладки {}: начало {motion_start:?}, предыдущая позиция {previous_top:?}, текущая {visible:?}, движение {animated}.",
                section.title,
            ),
        )?;
        if animated && section.rail == ALBUMS.rail {
            verify_indicator_phases(window, scene, output, previous_top, cx)?;
        }
        await_section(raw, section, cx).await?;
        settle_indicator(raw, animated, cx).await?;
        let tree = match capture {
            Some(capture) => frame(raw, scene, output, capture, cx)?,
            None => snapshot(raw, cx)?,
        };
        check_selection(&tree, section, expanded)?;
        let (top, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
        let at_destination =
            if section.rail == TRACKS.rail || (!expanded && section.rail != PLAYLISTS.rail) {
                top == tracks_top
            } else {
                top > previous_top
            };
        require(
            at_destination,
            &format!("Маркер должен остановиться на разделе {}.", section.title),
        )?;
        println!(
            "{}: {} marker {previous_top:?} → {top:?} (animation start {motion_start:?})",
            scene.name, section.title
        );
        previous_top = top;
        if section.rail == PLAYLISTS.rail {
            require(
                node(&tree, "playlist-import").is_some(),
                "Маршрут плейлистов должен показать действие импорта.",
            )?;
        }
    }

    press(raw, &named("home-search-page"), cx)?;
    wait(cx, "поиск из бокового меню", |cx| {
        let tree = snapshot(raw, cx)?;
        let ready = node_label(&tree, "music-search-title") == Some("Поиск")
            && toggled(&tree, "home-search-page", true)
            && [&TRACKS, &ALBUMS, &ARTISTS, &PLAYLISTS]
                .iter()
                .all(|section| {
                    node(&tree, section.rail).is_none() || toggled(&tree, section.rail, false)
                });
        Ok(ready.then_some(()))
    })
    .await?;
    press(raw, &named(TRACKS.rail), cx)?;
    let returned = await_section(raw, &TRACKS, cx).await?;
    check_selection(&returned, &TRACKS, expanded)?;
    require(
        queue_candidate(&returned).is_some() && toggled(&returned, "home-search-page", false),
        "Возврат в треки должен восстановить элементы списка и снять отметку поиска.",
    )?;
    if animated {
        settle_indicator(raw, animated, cx).await?;
        verify_indicator_retarget(window, scene, output, tracks_top, cx)?;
    }
    verify_search(window, scene, output, "home-search", "ctrl-k", cx).await?;
    verify_search(window, scene, output, "music-filter", "ctrl-f", cx).await?;
    println!(
        "{}: keyboard library routes, rail selection, search, clear and focus passed",
        scene.name
    );
    Ok(())
}

fn verify_indicator_phases(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    previous_top: Option<f32>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let mut positions = Vec::new();
    for percent in [0, 25, 50, 75, 100] {
        sample_indicator(window, Some(percent as f32 / 100.), cx)?;
        let tree = frame(
            raw,
            scene,
            output,
            &format!("nav-albums-marker-{percent:03}pct"),
            cx,
        )?;
        require(
            toggled(&tree, ALBUMS.rail, true)
                && toggled(&tree, TRACKS.rail, false)
                && focused(&tree) == Some(&named(ALBUMS.rail)),
            "До завершения движения маркера должны быть доступны выбранные Альбомы и фокус клавиатуры.",
        )?;
        positions.push(window.update(cx, |view, _, _| view.sidebar_motion_for_capture().0)?);
    }
    sample_indicator(window, None, cx)?;
    require(
        positions[0] == previous_top && positions.windows(2).all(|pair| pair[0] < pair[1]),
        &format!("Фазы движения маркера Альбомов должны последовательно расти: {positions:?}."),
    )?;
    println!(
        "{}: tracks → albums marker phases {positions:?}",
        scene.name
    );
    Ok(())
}

fn verify_indicator_retarget(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    tracks_top: Option<f32>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    press(raw, &named(ALBUMS.rail), cx)?;
    sample_indicator(window, Some(0.5), cx)?;
    frame(raw, scene, output, "nav-return-before-interruption", cx)?;
    focus(raw, &named(TRACKS.rail), cx)?;
    let (drawn, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    key(raw, "enter", cx)?;
    let (_, from) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    sample_indicator(window, Some(0.), cx)?;
    let reversed = frame(raw, scene, output, "nav-return-interrupted-start", cx)?;
    let (after, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    sample_indicator(window, Some(1.), cx)?;
    frame(raw, scene, output, "nav-return-interrupted-end", cx)?;
    let (settled, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    sample_indicator(window, None, cx)?;
    require(
        drawn > tracks_top
            && from == drawn
            && after == drawn
            && settled == tracks_top
            && toggled(&reversed, TRACKS.rail, true)
            && focused(&reversed) == Some(&named(TRACKS.rail)),
        &format!(
            "Прерванный переход должен вернуться в Треки с видимой позиции, сохранив фокус: {drawn:?} → {from:?} → {after:?} → {settled:?}.",
        ),
    )?;
    println!(
        "{}: interrupted albums marker {drawn:?} → {settled:?}",
        scene.name
    );
    Ok(())
}

fn sample_indicator(
    window: WindowHandle<MainView>,
    progress: Option<f32>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    window.update(cx, |view, _, cx| {
        view.sample_sidebar_motion_for_capture(progress, cx)
    })
}

async fn verify_search(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    id: &str,
    shortcut: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let clear = format!("{id}-clear");
    let focus = named(id);
    key(raw, shortcut, cx)?;
    if !scene.reduced {
        frame(raw, scene, output, &format!("{id}-focus-early"), cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(180))
            .await;
    }
    let empty = frame(raw, scene, output, &format!("{id}-focused"), cx)?;
    require(
        focused(&empty) == Some(&focus) && node(&empty, &clear).is_none(),
        "Клавиатурная команда должна сфокусировать пустое поле поиска.",
    )?;
    let library_filter = id == "music-filter";
    if library_filter {
        window.update(cx, |view, _, cx| {
            view.sample_collection_motion_for_capture(Some(0), cx);
        })?;
    }
    dispatch(
        raw,
        Keystroke {
            key: "z".into(),
            key_char: Some("z".into()),
            ..Default::default()
        },
        cx,
    )?;
    if library_filter {
        let mut alpha = Vec::new();
        for ms in [0, 55, 110, 165, 220] {
            window.update(cx, |view, _, cx| {
                view.sample_collection_motion_for_capture(Some(ms), cx);
            })?;
            let tree = frame(
                raw,
                scene,
                output,
                &format!("filter-row-departure-{ms:03}ms"),
                cx,
            )?;
            require(
                queue_candidate(&tree).is_none() && focused(&tree) == Some(&focus),
                "Исключённая строка должна исчезнуть из доступных действий и сохранить фокус поиска.",
            )?;
            alpha.push(window.update(cx, |view, _, _| view.library_departure_alpha_for_capture())?);
        }
        window.update(cx, |view, _, cx| {
            view.sample_collection_motion_for_capture(None, cx)
        })?;
        require(
            if scene.reduced {
                alpha.iter().all(|&value| value == 0.)
            } else {
                alpha[0] == 1. && alpha[4] == 0. && alpha.windows(2).all(|pair| pair[0] > pair[1])
            },
            &format!("Исключённые строки должны затухнуть в предыдущих позициях: {alpha:?}."),
        )?;
        println!("{} filtered rows departure alpha: {alpha:?}", scene.name);
    }
    if !scene.reduced {
        frame(raw, scene, output, &format!("{id}-typed-early"), cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(180))
            .await;
    }
    let typed = frame(raw, scene, output, &format!("{id}-typed"), cx)?;
    require(
        node(&typed, id).is_some_and(|field| field["aria"]["value"] == "z")
            && node_label(&typed, &clear) == Some("Очистить поиск"),
        "Введённый запрос и кнопка очистки должны появиться в дереве доступности.",
    )?;
    press(raw, &named(&clear), cx)?;
    let cleared = snapshot(raw, cx)?;
    require(
        focused(&cleared) == Some(&focus)
            && node(&cleared, id).is_some_and(|field| field["aria"]["value"] == "")
            && node(&cleared, &clear).is_none(),
        "Очистка должна убрать запрос и вернуть фокус полю.",
    )?;
    key(raw, "escape", cx)
}

async fn await_section(
    raw: AnyWindowHandle,
    section: &Section,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    wait(cx, section.title, |cx| {
        let tree = snapshot(raw, cx)?;
        let ready = node_label(&tree, "library-counts") == Some(section.count)
            && (section.rail != TRACKS.rail || queue_candidate(&tree).is_some());
        Ok(ready.then_some(tree))
    })
    .await
}

fn check_selection(tree: &Value, current: &Section, expanded: bool) -> gpui::Result<()> {
    let tracks = current.rail == TRACKS.rail;
    let heading = if tracks {
        "music-liked-title"
    } else {
        "library-title"
    };
    let title = if expanded && !tracks {
        current.title
    } else {
        "Медиатека"
    };
    require(
        node_label(tree, heading) == Some(title)
            && node(tree, "library-tabs").is_some() != expanded,
        &format!(
            "Раздел {} должен отобразить свой заголовок и навигацию.",
            current.title
        ),
    )?;
    let active_rail = if expanded || current.rail == PLAYLISTS.rail {
        current.rail
    } else {
        TRACKS.rail
    };
    for section in [&TRACKS, &ALBUMS, &ARTISTS, &PLAYLISTS] {
        if expanded || matches!(section.rail, "studio-library" | "home-playlists") {
            let label = if !expanded && section.rail == TRACKS.rail {
                "Медиатека"
            } else {
                section.title
            };
            require(
                node_label(tree, section.rail) == Some(label)
                    && toggled(tree, section.rail, section.rail == active_rail),
                &format!(
                    "Неверное имя или отметка в боковом меню для {}.",
                    section.rail
                ),
            )?;
        } else {
            require(
                node(tree, section.rail).is_none(),
                &format!("Компактное меню не должно дублировать {}.", section.title),
            )?;
        }
        if !expanded {
            require(
                node_label(tree, section.tab) == Some(section.title)
                    && node(tree, section.tab).is_some_and(|tab| {
                        tab["aria"]["role"] == "Tab"
                            && tab["aria"]["selected"].as_bool() == Some(section.tab == current.tab)
                    }),
                &format!("Вкладка {} должна отмечать свой маршрут.", section.title),
            )?;
        }
    }
    Ok(())
}

fn toggled(tree: &Value, id: &str, active: bool) -> bool {
    node(tree, id).is_some_and(|item| {
        item["aria"]["role"] == "Button"
            && item["aria"]["toggled"] == if active { "True" } else { "False" }
    })
}

async fn settle_indicator(
    raw: AnyWindowHandle,
    animated: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    if animated {
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        cx.background_executor()
            .timer(Duration::from_millis(260))
            .await;
    }
    Ok(())
}
