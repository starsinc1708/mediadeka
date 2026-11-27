use std::{fs, io, path::Path, time::Duration};

use gpui::{
    AnyWindowHandle, AsyncApp, Bounds, InputEvent as _, Pixels, ScrollDelta, ScrollWheelEvent,
    WindowHandle, point, px,
};
use mediadeka::MainView;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::scroll_viewport::Surface as ScrollSurface;

use super::{
    COVER, SCENES, Scene, details, fixture, focused, frame, key, named, node, node_label, press,
    require, shader_with_artwork_palette, snapshot, sound, wait,
};

const ARTIST_SLOT: &str = "artist:ArtistPortraitLarge001";
const ARTIST_URL: &str = "https://open.spotify.com/artist/ArtistPortraitLarge001";
const ARTIST: &str = "Северный ансамбль";
const ARTIST_ART: &str = "https://i.scdn.co/image/artist-hero-large";
const SINGLE_URL: &str = "https://open.spotify.com/album/FluidShortSingle001234";
const SINGLE: &str = "Ледяной берег";
const SINGLE_ART: &str = "https://i.scdn.co/image/stage-single";
const LONG_PLAYLIST_ID: &str = "collapsing-hero-playlist-id";
const TRACK_COUNT: usize = 48;

#[derive(Clone, Copy)]
enum Surface {
    Scroll(ScrollSurface),
    LocalGroup,
}

impl Surface {
    fn bounds(self, view: &MainView) -> (Bounds<Pixels>, f32, f32) {
        match self {
            Self::Scroll(surface) => surface.bounds(view),
            Self::LocalGroup => view.library_scroll_bounds_for_capture(),
        }
    }
}

pub(super) fn seed(profile: &Path) -> gpui::Result<()> {
    // Distinct paths keep all the WAVs visible to the local album/artist index.
    let source = profile.join("track-7.wav");
    let mut paths = Vec::with_capacity(TRACK_COUNT);
    for index in 0..TRACK_COUNT {
        let track = profile.join(format!("hero-track-{index:02}.wav"));
        fs::hard_link(&source, &track)?;
        paths.push(track);
    }
    let playlist = mediadeka::seed_playlist_capture(profile, &paths)?;
    fs::write(profile.join(LONG_PLAYLIST_ID), playlist.to_string())?;
    let database = Connection::open(profile.join("mediadeka.sqlite3"))?;
    let payload: String = database.query_row(
        "SELECT payload FROM music_snapshot WHERE provider = 'spotify' AND profile_id = ''
         AND region = ?1 AND slot = ?2",
        params![details::ARTIST_REGION, ARTIST_SLOT],
        |row| row.get(0),
    )?;
    let mut artist: Value = serde_json::from_str(&payload)?;
    let sample_track = artist["tracks"][0].clone();
    artist["tracks"] = Value::Array(
        (1..=TRACK_COUNT)
            .map(|index| {
                let mut track = sample_track.clone();
                track["id"] = json!(format!("hero-artist-track-{index:02}"));
                track["title"] = json!(format!("Поворот реки {index:02}"));
                track
            })
            .collect(),
    );
    database.execute(
        "UPDATE music_snapshot SET payload = ?1 WHERE provider = 'spotify' AND profile_id = ''
         AND region = ?2 AND slot = ?3",
        params![artist.to_string(), details::ARTIST_REGION, ARTIST_SLOT],
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Hero {
    heading: &'static str,
    play: &'static str,
    group: &'static str,
    columns: Option<&'static str>,
    surface: Surface,
    secondary: Option<&'static str>,
    secondary_controls: &'static [&'static str],
}

const OBJECT: Hero = Hero {
    heading: "music-object-title",
    play: "music-object-play",
    group: "music-object-hero",
    columns: None,
    surface: Surface::Scroll(ScrollSurface::Workspace),
    secondary: Some("music-object-secondary-actions"),
    secondary_controls: &["music-object-shuffle", "music-object-hero-menu"],
};
const ALBUM: Hero = Hero {
    columns: Some("music-object-columns"),
    ..OBJECT
};
const PLAYLIST: Hero = Hero {
    heading: "playlist-title",
    play: "playlist-play",
    group: "playlist-media-hero",
    columns: Some("playlist-track-columns"),
    surface: Surface::Scroll(ScrollSurface::Playlist),
    secondary: Some("playlist-hero-secondary-actions"),
    secondary_controls: &["playlist-shuffle", "pin-toggle", "playlist-more"],
};
const LOCAL_GROUP: Hero = Hero {
    heading: "music-liked-title",
    play: "library-group-play",
    group: "library-group-hero",
    columns: Some("library-group-columns"),
    surface: Surface::LocalGroup,
    secondary: None,
    secondary_controls: &[],
};

#[derive(Clone, Copy)]
struct Pose {
    offset: f32,
    extent: f32,
    cover: Bounds<Pixels>,
}

impl Pose {
    fn edge(self) -> f32 {
        self.cover.size.width.into()
    }

    fn report(self) -> Value {
        json!({
            "offset_dip": self.offset,
            "extent_dip": self.extent,
            "cover_dip": [
                f32::from(self.cover.left()), f32::from(self.cover.top()),
                self.edge(), f32::from(self.cover.size.height),
            ],
        })
    }
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    profile: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let immediate = scene.reduced || high_contrast;
    let mut pages = serde_json::Map::new();
    open(
        window,
        details::ALBUM_URL,
        details::ALBUM_TITLE,
        details::ALBUM_ART,
        cx,
    )
    .await?;
    let album = capture_cycle(window, scene, output, "album", ALBUM, immediate, cx)?;
    if scene.width == 1920. {
        if !immediate {
            capture_replay(window, scene, output, ALBUM, cx)?;
        }
        pages.insert(
            "dock_and_resize".into(),
            dock_and_resize(window, scene, output, ALBUM, cx)?,
        );
        pages.insert(
            "compact_playback".into(),
            play_and_pause(window, scene, output, cx).await?,
        );
    }
    wheel(window, ALBUM.surface, -420., cx)?;
    press(raw, &named("music-object-title-top"), cx)?;
    wait(
        cx,
        "возврат к началу по заголовку",
        |cx| {
            snapshot(raw, cx)?;
            let returned = pose(window, ALBUM.surface, cx)?;
            Ok((returned.offset.abs() <= 1.).then_some(()))
        },
    )
    .await?;
    let title_returned = shot(window, scene, output, "album-title-returned", ALBUM, cx)?;

    wheel(window, OBJECT.surface, -420., cx)?;
    snapshot(raw, cx)?;
    let leaving_album = pose(window, OBJECT.surface, cx)?.offset;
    open(window, ARTIST_URL, ARTIST, ARTIST_ART, cx).await?;
    wheel(window, OBJECT.surface, -1200., cx)?;
    press(raw, &named("music-artist-show-tracks"), cx)?;
    pages.insert(
        "artist".into(),
        capture_cycle(window, scene, output, "artist", OBJECT, immediate, cx)?,
    );
    press(raw, &named("workspace-back"), cx)?;
    ready(window, OBJECT, details::ALBUM_TITLE, cx).await?;
    wait(
        cx,
        "восстановление позиции альбома",
        |cx| {
            snapshot(raw, cx)?;
            let returned = pose(window, OBJECT.surface, cx)?;
            Ok(((returned.offset - leaving_album).abs() <= 1.).then_some(returned))
        },
    )
    .await?;
    let returned = shot(window, scene, output, "album-back", ALBUM, cx)?;
    pages.insert(
        "album".into(),
        json!({ "cycle": album, "title_return": title_returned.report(), "before_back_dip": leaving_album, "back": returned.report() }),
    );

    open(window, SINGLE_URL, SINGLE, SINGLE_ART, cx).await?;
    let short = shot(window, scene, output, "single-expanded", OBJECT, cx)?;
    wheel(window, OBJECT.surface, -1600., cx)?;
    let end = shot(window, scene, output, "single-end", OBJECT, cx)?;
    require(
        (end.extent - end.offset).abs() <= 1.,
        "Последняя строка короткого релиза должна оставаться доступной.",
    )?;
    pages.insert(
        "single".into(),
        json!({"expanded": short.report(), "end": end.report()}),
    );

    let id: i64 = fs::read_to_string(profile.join(LONG_PLAYLIST_ID))?
        .trim()
        .parse()
        .map_err(io::Error::other)?;
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(id, window, cx)
    })?;
    wait(
        cx,
        "длинный локальный плейлист",
        |cx| {
            let tree = snapshot(raw, cx)?;
            let count = window.update(cx, |view, _, _| view.playlist_status_for_capture())?;
            Ok(
                (matches!(count, Some(Ok(TRACK_COUNT))) && node(&tree, PLAYLIST.play).is_some())
                    .then_some(()),
            )
        },
    )
    .await?;
    let playlist = capture_cycle(window, scene, output, "playlist", PLAYLIST, immediate, cx)?;
    wheel(window, PLAYLIST.surface, -720., cx)?;
    press(raw, &named("playlist-more"), cx)?;
    let menu = frame(raw, scene, output, "playlist-compact-menu", cx)?;
    require(
        node(&menu, "playlist-menu").is_some(),
        "Дополнительные действия должны открываться из компактного Hero.",
    )?;
    key(raw, "escape", cx)?;
    let dismissed = snapshot(raw, cx)?;
    require(
        node(&dismissed, "playlist-menu").is_none()
            && focused(&dismissed) == Some(named("playlist-more").as_str()),
        "Escape должен вернуть фокус кнопке дополнительных действий.",
    )?;
    pages.insert(
        "playlist".into(),
        json!({ "cycle": playlist, "compact_menu": true }),
    );

    for (name, rail, tab, row, title) in [
        (
            "local-album",
            "studio-library-albums",
            "library-tab-albums",
            "library-album-",
            "Горизонт",
        ),
        (
            "local-artist",
            "studio-library-artists",
            "library-tab-artists",
            "library-row-",
            "Город во сне",
        ),
    ] {
        window.update(cx, |view, window, cx| {
            view.show_music_collection_for_capture(false, window, cx);
        })?;
        let tree = snapshot(raw, cx)?;
        press(
            raw,
            &named(if node(&tree, rail).is_some() {
                rail
            } else {
                tab
            }),
            cx,
        )?;
        let tile = wait(cx, title, |cx| {
            let tree = snapshot(raw, cx)?;
            Ok((0..8).find_map(|index| {
                let id = format!("{row}{index}");
                node_label(&tree, &id)
                    .is_some_and(|label| label.starts_with(title))
                    .then_some(id)
            }))
        })
        .await?;
        press(raw, &named(&tile), cx)?;
        ready(window, LOCAL_GROUP, title, cx).await?;
        pages.insert(
            name.into(),
            capture_cycle(window, scene, output, name, LOCAL_GROUP, immediate, cx)?,
        );
        press(raw, &named("workspace-back"), cx)?;
        wait(cx, "выход из группы медиатеки", |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(node(&tree, LOCAL_GROUP.group).is_none().then_some(()))
        })
        .await?;
    }

    fs::write(
        output.join("collapsing-hero.json"),
        serde_json::to_vec_pretty(&json!({
            "viewport_dip": [scene.width, scene.height],
            "dpi": scene.dpi,
            "reduced_motion": scene.reduced,
            "high_contrast": high_contrast,
            "pages": pages,
        }))?,
    )?;
    if !high_contrast && shader_with_artwork_palette() {
        capture_playlist_palette_edges(window, scene, output, id, cx).await?;
    }
    Ok(())
}

async fn capture_playlist_palette_edges(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    id: i64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const PHASE: Duration = Duration::from_secs(4);
    const DOCK_TIME: Duration = Duration::from_secs(8);
    let raw = window.into();
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_with_artwork_for_capture(id, Some(COVER), window, cx);
        view.set_fluid_glass_cover_for_capture(fixture::WARM_COVER, cx);
        view.sample_fluid_glass_palette_for_capture(Duration::from_secs(120), PHASE, cx);
    })?;
    let page_palette = wait(
        cx,
        "обложка и палитра плейлиста",
        |cx| {
            let tree = snapshot(raw, cx)?;
            window.update(cx, |view, _, cx| {
                (node(&tree, PLAYLIST.play).is_some()
                    && view.artist_artwork_decoded_for_capture(COVER, cx)
                    && view.fluid_glass_palette_ready_for_capture(fixture::WARM_COVER, cx))
                .then(|| view.fluid_glass_page_palette_for_capture(cx))
                .flatten()
            })
        },
    )
    .await?;
    for wall in [124, 128] {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_palette_for_capture(Duration::from_secs(wall), PHASE, cx);
        })?;
        snapshot(raw, cx)?;
    }
    let player_palette = window.update(cx, |view, _, cx| {
        view.fluid_glass_target_colors_for_capture(cx)
    })?;
    require(
        page_palette != player_palette,
        "Фон плейлиста и акцент плеера должны использовать разные палитры.",
    )?;

    let mut frames = Vec::new();
    for docked in [false, true]
        .into_iter()
        .take(if scene.width == 1920. { 2 } else { 1 })
    {
        if docked {
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_dock_for_capture(Some(DOCK_TIME), cx);
            })?;
            press(raw, &named("player-queue"), cx)?;
            snapshot(raw, cx)?;
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_dock_for_capture(
                    Some(DOCK_TIME + Duration::from_secs(1)),
                    cx,
                );
            })?;
        }
        let stem = if docked {
            "playlist-palette-docked"
        } else {
            "playlist-palette"
        };
        let tree = frame(raw, scene, output, stem, cx)?;
        let ([inset, rail, player, inner_radius, ..], (dock_width, right)) =
            window.update(cx, |view, window, _| {
                (
                    view.fluid_glass_geometry_for_capture(window),
                    view.dock_splitter_for_capture(window),
                )
            })?;
        let pose = pose(window, PLAYLIST.surface, cx)?;
        require(
            node(&tree, PLAYLIST.play).is_some()
                && (!docked || (node(&tree, "right-panel-resize").is_some() && dock_width > 0.)),
            "Кадр должен показать плейлист и запрошенное состояние правой панели.",
        )?;
        frames.push(json!({
            "frame": format!("{stem}.png"),
            "dock_width_dip": dock_width,
            "inset_dip": inset,
            "rail_dip": rail,
            "player_dip": player,
            "inner_radius_dip": inner_radius,
            "content_left_dip": inset + rail,
            "content_right_dip": f32::from(right),
            "content_bottom_dip": scene.height - inset - player,
            "playlist": pose.report(),
        }));
    }
    fs::write(
        output.join("playlist-palette.json"),
        serde_json::to_vec_pretty(&json!({
            "viewport_dip": [scene.width, scene.height],
            "dpi": scene.dpi,
            "shader_phase_ms": PHASE.as_millis() as u64,
            "page_palette": page_palette,
            "player_palette": player_palette,
            "frames": frames,
        }))?,
    )?;
    // The next scene reuses this isolated profile and starts from its route.
    window.update(cx, |view, window, cx| {
        view.sample_fluid_glass_dock_for_capture(None, cx);
        view.resume_fluid_glass_for_capture(cx);
        view.close_right_panel_for_capture(window, cx);
        view.show_music_collection_for_capture(false, window, cx);
    })?;
    Ok(())
}

async fn open(
    window: WindowHandle<MainView>,
    url: &str,
    title: &str,
    artwork: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    println!("Открываем страницу «{title}»");
    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(url, window, cx);
    })?;
    ready(window, OBJECT, title, cx).await?;
    println!("Заголовок «{title}» доступен");
    wait(cx, "обложка Hero", |cx| {
        snapshot(window.into(), cx)?;
        let decoded = window.update(cx, |view, _, cx| {
            view.artist_artwork_decoded_for_capture(artwork, cx)
        })?;
        Ok(decoded.then_some(()))
    })
    .await
}

async fn ready(
    window: WindowHandle<MainView>,
    hero: Hero,
    title: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait(cx, title, |cx| {
        let tree = snapshot(window.into(), cx)?;
        Ok(
            (node_label(&tree, hero.heading) == Some(title) && node(&tree, hero.play).is_some())
                .then_some(()),
        )
    })
    .await
}

fn capture_cycle(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    name: &str,
    hero: Hero,
    immediate: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    wheel(window, hero.surface, 8000., cx)?;
    let initial = shot(window, scene, output, &format!("{name}-expanded"), hero, cx)?;
    let delta = (initial.edge() * 0.42).clamp(30., 140.);
    wheel(window, hero.surface, -delta, cx)?;
    let midway = shot(window, scene, output, &format!("{name}-midway"), hero, cx)?;
    if immediate && name == "album" {
        let tree = snapshot(window.into(), cx)?;
        require(
            node(&tree, "music-confirmed-links").is_none(),
            "В дискретной шапке дополнительные ссылки скрываются целиком при прокрутке.",
        )?;
    }
    wheel(window, hero.surface, -650., cx)?;
    let compact = shot(window, scene, output, &format!("{name}-compact"), hero, cx)?;
    require(
        initial.offset.abs() <= 1.
            && midway.offset > initial.offset
            && compact.offset > midway.offset
            && initial.edge() > compact.edge()
            && (39.5..=48.5).contains(&compact.edge()),
        &format!(
            "Некорректное сворачивание {name}: {} / {} / {}.",
            initial.report(),
            midway.report(),
            compact.report()
        ),
    )?;
    let middle_geometry = if immediate {
        midway.edge() <= initial.edge() + 1. && midway.edge() > compact.edge() + 1.
    } else {
        initial.edge() > midway.edge() && midway.edge() > compact.edge()
    };
    require(
        middle_geometry,
        &format!(
            "Промежуточный размер обложки {name} не соответствует режиму движения: {}.",
            midway.report()
        ),
    )?;
    wheel(window, hero.surface, compact.offset - midway.offset, cx)?;
    let reverse = shot(window, scene, output, &format!("{name}-reverse"), hero, cx)?;
    require(
        (midway.offset - reverse.offset).abs() <= 1.
            && (midway.edge() - reverse.edge()).abs() <= 1.,
        &format!(
            "Путь Hero {name} зависит от направления прокрутки: {} / {}.",
            midway.report(),
            reverse.report()
        ),
    )?;
    wheel(window, hero.surface, 8000., cx)?;
    let returned = shot(window, scene, output, &format!("{name}-returned"), hero, cx)?;
    require(
        returned.offset.abs() <= 1. && (initial.edge() - returned.edge()).abs() <= 1.,
        "Прокрутка вверх должна вернуть развёрнутую геометрию.",
    )?;
    let cover_signal = if name == "album" {
        [
            ("album-expanded", initial),
            ("album-midway", midway),
            ("album-compact", compact),
            ("album-reverse", reverse),
            ("album-returned", returned),
        ]
        .into_iter()
        .map(|(frame, pose)| {
            let image = image::open(output.join(format!("{frame}.png")))?.into_rgba8();
            Ok((frame, cyan_fraction(&image, pose, scene)))
        })
        .collect::<gpui::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    require(
        cover_signal.iter().all(|(_, signal)| *signal > 0.018),
        &format!("Обложка альбома потеряла бирюзовый рисунок при прокрутке: {cover_signal:?}."),
    )?;
    Ok(json!({
        "expanded": initial.report(), "midway": midway.report(), "compact": compact.report(),
        "reverse": reverse.report(), "returned": returned.report(),
        "midway_stays_expanded": middle_geometry && immediate,
        "fixture_artwork_cyan_fraction": cover_signal,
    }))
}

fn cyan_fraction(frame: &image::RgbaImage, pose: Pose, scene: Scene) -> f32 {
    let scale = scene.dpi as f32 / 96.;
    let left = f32::from(pose.cover.left());
    let top = f32::from(pose.cover.top());
    let edge = pose.edge();
    let mut cyan = 0;
    let mut total = 0;
    // Keep the sample inside the rounded corners and scale it with the cover.
    for y in 1..19 {
        for x in 1..19 {
            let px = ((left + edge * x as f32 / 20.) * scale).round() as u32;
            let py = ((top + edge * y as f32 / 20.) * scale).round() as u32;
            if let Some(image::Rgba([r, g, b, _])) = frame.get_pixel_checked(px, py).copied() {
                total += 1;
                if g > 64
                    && g.saturating_sub(r) > 25
                    && b.saturating_sub(r) > 20
                    && f32::from(g) > 0.75 * f32::from(b)
                {
                    cyan += 1;
                }
            }
        }
    }
    cyan as f32 / total.max(1) as f32
}

fn capture_replay(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    hero: Hero,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let directory = output.join("album-replay");
    fs::create_dir_all(&directory)?;
    let mut poses = Vec::with_capacity(49);
    let mut artwork = Vec::with_capacity(49);
    for index in 0..=48 {
        if index > 0 {
            wheel(
                window,
                hero.surface,
                if index <= 24 { -16. } else { 16. },
                cx,
            )?;
        }
        let image = crate::render_image(window.into(), scene, cx)?;
        let pose = pose(window, hero.surface, cx)?;
        image.save(directory.join(format!("frame-{index:03}.png")))?;
        artwork.push(cyan_fraction(&image, pose, scene));
        poses.push(pose);
    }
    require(
        poses[..=24]
            .windows(2)
            .all(|pair| pair[1].edge() <= pair[0].edge() + 0.5)
            && poses[24..]
                .windows(2)
                .all(|pair| pair[1].edge() + 0.5 >= pair[0].edge())
            && (39.5..=48.5).contains(&poses[24].edge())
            && poses[48].offset.abs() <= 1.,
        "Нативная последовательность должна непрерывно свернуть Hero и раскрыть его назад.",
    )?;
    fs::write(
        directory.join("poses.json"),
        serde_json::to_vec_pretty(&json!({
            "positions": poses.into_iter().map(Pose::report).collect::<Vec<_>>(),
            "fixture_artwork_cyan_fraction": artwork,
        }))?,
    )?;
    require(
        artwork.iter().all(|signal| *signal > 0.018),
        "В нативной записи обложка прерывается на одном из переходных кадров.",
    )?;
    println!(
        "{}: сохранено 49 кадров прокрутки в обе стороны",
        scene.name
    );
    Ok(())
}

fn dock_and_resize(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    hero: Hero,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = window.into();
    wheel(window, hero.surface, -460., cx)?;
    let wide = shot(window, scene, output, "album-wide", hero, cx)?;
    press(raw, &named("player-queue"), cx)?;
    let docked_tree = frame(raw, scene, output, "album-docked", cx)?;
    let docked = pose(window, hero.surface, cx)?;
    require(
        node(&docked_tree, "right-panel-resize").is_some()
            && node(&docked_tree, hero.play).is_some()
            && (wide.offset - docked.offset).abs() <= 1.,
        "Открытие пристыкованной панели должно сохранять действия Hero и прокрутку.",
    )?;
    key(raw, "escape", cx)?;
    let compact_scene = Scene {
        reduced: scene.reduced,
        ..SCENES[1]
    };
    raw.update(cx, |_, window, _| {
        crate::native_dpi_change(window, compact_scene)
    })??
    .apply();
    let resized = shot(window, compact_scene, output, "album-resized", hero, cx)?;
    require(
        resized.offset > 0. && (39.5..=48.5).contains(&resized.edge()),
        "При уменьшении окна прокрученный Hero должен оставаться свёрнутым.",
    )?;
    raw.update(cx, |_, window, _| crate::native_dpi_change(window, scene))??
        .apply();
    let restored = shot(window, scene, output, "album-restored-size", hero, cx)?;
    require(
        (restored.offset - wide.offset).abs() <= 1.,
        "Изменение ширины не должно сбрасывать позицию длинного альбома.",
    )?;
    Ok(json!({
        "wide": wide.report(), "docked": docked.report(),
        "resized": resized.report(), "restored": restored.report(),
    }))
}

async fn play_and_pause(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = window.into();
    press(raw, &named(ALBUM.play), cx)?;
    let playback = sound(window, "Течение 01", cx).await?;
    let playing = frame(raw, scene, output, "album-compact-playing", cx)?;
    require(
        node_label(&playing, ALBUM.play) == Some("Пауза"),
        "Кнопка компактного Hero должна переключиться на паузу.",
    )?;
    press(raw, &named(ALBUM.play), cx)?;
    let paused = wait(cx, "пауза через компактный Hero", |cx| {
        let (_, _, status, position) =
            window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
        Ok((status == "paused").then_some(position))
    })
    .await?;
    frame(raw, scene, output, "album-compact-paused", cx)?;
    Ok(json!({"playback": playback, "pause_seconds": paused}))
}

fn shot(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    name: &str,
    hero: Hero,
    cx: &mut AsyncApp,
) -> gpui::Result<Pose> {
    let tree = frame(window.into(), scene, output, name, cx)?;
    let duplicates = tree["nodes"].as_object().map_or(0, |nodes| {
        let heading = named(hero.heading);
        nodes
            .values()
            .filter(|node| node["element_id"] == heading)
            .count()
    });
    let clustered = hero.secondary.is_none_or(|id| {
        node(&tree, id).is_some_and(|group| group["aria"]["role"] == "Group")
            && hero.secondary_controls.iter().all(|control| {
                node(&tree, control).is_some_and(|node| {
                    node["aria"]["label"]
                        .as_str()
                        .is_some_and(|label| !label.is_empty())
                })
            })
    });
    require(
        duplicates == 1
            && node(&tree, hero.group).is_some()
            && node(&tree, hero.play).is_some()
            && clustered,
        &format!(
            "Кадр {name} должен показывать один Hero с названием, действием и доступной группой второстепенных команд."
        ),
    )?;
    let actual = pose(window, hero.surface, cx)?;
    require(
        (actual.edge() - f32::from(actual.cover.size.height)).abs() <= 1.,
        &format!(
            "Обложка Hero в кадре {name} должна быть квадратной: {}.",
            actual.report()
        ),
    )?;
    if scene.width == 1920. && (39.5..=48.5).contains(&actual.edge()) {
        require(
            hero.columns.is_none_or(|id| node(&tree, id).is_some()),
            &format!("В компактном Hero {name} должны оставаться заголовки колонок."),
        )?;
    }
    Ok(actual)
}

fn pose(window: WindowHandle<MainView>, surface: Surface, cx: &mut AsyncApp) -> gpui::Result<Pose> {
    window.update(cx, |view, window, _| {
        let (_, offset, extent) = surface.bounds(view);
        let cover = view
            .rendered_hero_artwork_for_capture(window)
            .ok_or_else(|| io::Error::other("В Hero отсутствуют границы обложки."))?;
        Ok(Pose {
            offset,
            extent,
            cover,
        })
    })?
}

fn wheel(
    window: WindowHandle<MainView>,
    surface: Surface,
    delta: f32,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let (bounds, _, _) = window.update(cx, |view, _, _| surface.bounds(view))?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position: point(bounds.center().x, bounds.bottom() - px(16.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
    })
}
