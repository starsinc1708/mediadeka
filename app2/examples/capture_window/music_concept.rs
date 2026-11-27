#[path = "music_concept/layout.rs"]
mod layout;

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyWindowHandle, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle,
    InputEvent as _, KeyUpEvent, Keystroke, RenderImage, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, Subscription, TextRenderingMode, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, point, px, size,
};
use guise::TextInput;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Scene, draw, fail, native_dpi_change, sound};

const DESKTOP: Scene = Scene {
    name: "1920x1080",
    width: 1920.,
    height: 1080.,
    dpi: 96,
    reduced: true,
};
const COMPACT: Scene = Scene {
    name: "800x400",
    width: 800.,
    height: 400.,
    ..DESKTOP
};

#[derive(Deserialize)]
struct Catalog {
    releases: Vec<ReleaseInfo>,
}

#[derive(Deserialize)]
struct ReleaseInfo {
    title: String,
    artist: String,
    year: Option<String>,
    cover: PathBuf,
    tracks: Vec<Track>,
}

#[derive(Deserialize)]
struct Track {
    title: String,
    artist: String,
    duration: String,
}

impl Track {
    fn duration_seconds(&self) -> u32 {
        self.duration
            .split_once(':')
            .and_then(|(minutes, seconds)| {
                Some(
                    minutes
                        .parse::<u32>()
                        .ok()?
                        .saturating_mul(60)
                        .saturating_add(seconds.parse::<u32>().ok()?),
                )
            })
            .unwrap_or(0)
    }
}

struct Release {
    info: ReleaseInfo,
    image: Arc<RenderImage>,
    background: Arc<RenderImage>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TrackId {
    release: usize,
    index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Browse {
    Library,
    Search,
    Release(TrackId),
}

struct Concept {
    releases: Arc<[Release]>,
    browse: Browse,
    release_from_search: bool,
    query: Entity<TextInput>,
    _search_changed: Subscription,
    queue_open: bool,
    current: TrackId,
    following: Vec<TrackId>,
    previous: Vec<TrackId>,
    paused: bool,
    position_seconds: u32,
    volume_percent: u8,
    focus: FocusHandle,
    track_scroll: ScrollHandle,
    row_focus: Vec<Vec<FocusHandle>>,
}

impl Concept {
    fn new(releases: Arc<[Release]>, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            TextInput::new(cx)
                .id("concept-query")
                .accessible_name("Поиск по медиатеке")
                .placeholder("Название, исполнитель или трек")
                .tab_order(10)
                .bare(true)
        });
        let search_changed = cx.subscribe(&query, |_, _, _, cx| cx.notify());
        let position_seconds = 34.min(releases[0].info.tracks[0].duration_seconds());
        let row_focus = releases
            .iter()
            .map(|release| {
                release
                    .info
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        cx.focus_handle()
                            .tab_index(100 + index as isize)
                            .tab_stop(true)
                    })
                    .collect()
            })
            .collect();
        Self {
            releases,
            browse: Browse::Library,
            release_from_search: false,
            query,
            _search_changed: search_changed,
            queue_open: false,
            current: TrackId {
                release: 0,
                index: 0,
            },
            following: (1..3).map(|index| TrackId { release: 0, index }).collect(),
            previous: Vec::new(),
            paused: false,
            position_seconds,
            volume_percent: 72,
            focus: cx.focus_handle(),
            track_scroll: ScrollHandle::new(),
            row_focus,
        }
    }

    fn track(&self, id: TrackId) -> &Track {
        &self.releases[id.release].info.tracks[id.index]
    }

    fn add_next(&mut self, track: TrackId) {
        self.following.insert(0, track);
    }

    fn play(&mut self, track: TrackId) {
        if self.current != track {
            self.previous.push(self.current);
        }
        self.current = track;
        self.position_seconds = 0;
        self.paused = false;
    }

    fn duration_seconds(&self) -> u32 {
        self.track(self.current).duration_seconds()
    }

    fn has_previous(&self) -> bool {
        !self.previous.is_empty() || self.position_seconds > 0
    }

    fn previous_track(&mut self) {
        if let Some(track) = self.previous.pop() {
            self.following.insert(0, self.current);
            self.current = track;
        }
        self.position_seconds = 0;
    }

    fn start_release(&mut self, track: TrackId) {
        self.play(track);
        let count = self.releases[track.release].info.tracks.len();
        self.following = (track.index + 1..count)
            .map(|index| TrackId { index, ..track })
            .collect();
    }

    fn next(&mut self) {
        if !self.following.is_empty() {
            let track = self.following.remove(0);
            self.play(track);
        }
    }

    fn back(&mut self) {
        if self.queue_open {
            self.queue_open = false;
        } else if matches!(self.browse, Browse::Release(_)) && self.release_from_search {
            self.browse = Browse::Search;
        } else {
            self.browse = Browse::Library;
        }
    }

    fn open_release(&mut self, track: TrackId) {
        self.release_from_search = self.browse == Browse::Search;
        self.browse = Browse::Release(track);
        self.track_scroll.set_offset(point(px(0.), px(0.)));
        self.track_scroll.scroll_to_item(track.index);
    }

    fn open_search(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.queue_open = false;
        self.browse = Browse::Search;
        self.query.update(cx, |field, cx| field.focus(window, cx));
        cx.notify();
    }

    fn focused_row(&self, window: &gpui::Window, cx: &gpui::App) -> Option<(usize, usize)> {
        let Browse::Release(track) = self.browse else {
            return None;
        };
        if self.queue_open {
            return None;
        }
        let focus = window.focused(cx)?;
        self.row_focus[track.release]
            .iter()
            .position(|handle| *handle == focus)
            .map(|index| (track.release, index))
    }
}

fn remaining_track_scroll(scroll: &ScrollHandle) -> gpui::Pixels {
    (scroll.max_offset().y + scroll.offset().y).max(gpui::Pixels::ZERO)
}

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let usage = || io::Error::other("--music-concept <выход|preview> <catalog.json>");
    let output = PathBuf::from(args.next().ok_or_else(usage)?);
    let catalog = PathBuf::from(args.next().ok_or_else(usage)?);
    if args.next().is_some() {
        return Err(usage().into());
    }
    let visible = output.as_os_str() == "preview";
    let releases = load_catalog(&catalog)?;
    mediadeka::application().run(move |cx| {
        cx.set_text_rendering_mode(TextRenderingMode::Grayscale);
        layout::install_theme(cx).unwrap_or_else(|error| fail(error));
        if visible {
            open(cx, DESKTOP, releases, true).unwrap_or_else(|error| fail(error));
        } else {
            cx.spawn(async move |cx| {
                for scene in [DESKTOP, COMPACT] {
                    let result =
                        capture(cx, scene, &output.join(scene.name), releases.clone()).await;
                    if let Err(error) = result {
                        fail(error);
                    }
                }
            })
            .detach();
        }
    });
    Ok(())
}

fn load_catalog(path: &Path) -> gpui::Result<Arc<[Release]>> {
    let catalog: Catalog = serde_json::from_slice(&fs::read(path)?)?;
    if catalog.releases.len() < 2
        || catalog
            .releases
            .iter()
            .any(|release| release.tracks.len() < 3)
    {
        return Err(io::Error::other("Нужны два релиза с тремя или более треками.").into());
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let releases = catalog
        .releases
        .into_iter()
        .map(|info| -> gpui::Result<_> {
            if info.cover.components().count() != 1 || info.cover.file_name().is_none() {
                return Err(
                    io::Error::other("Обложка должна быть файлом рядом с каталогом.").into(),
                );
            }
            let pixels = image::open(dir.join(&info.cover))?.into_rgba8();
            let thumbnail = image::imageops::thumbnail(&pixels, 240, 240);
            let background = image::imageops::blur(&thumbnail, 12.);
            Ok(Release {
                info,
                image: render_image(pixels),
                background: render_image(background),
            })
        })
        .collect::<gpui::Result<Vec<_>>>()?;
    Ok(releases.into())
}

fn render_image(mut pixels: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}

fn open(
    cx: &mut gpui::App,
    scene: Scene,
    releases: Arc<[Release]>,
    visible: bool,
) -> gpui::Result<WindowHandle<Concept>> {
    let bounds = Bounds::centered(None, size(px(scene.width), px(scene.height)), cx);
    cx.open_window(
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Mediadeka · Прототип".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(COMPACT.width), px(COMPACT.height))),
            show: visible,
            focus: visible,
            ..Default::default()
        },
        move |window, cx| {
            if !visible {
                window.resize(bounds.size);
            }
            let view = cx.new(|cx| Concept::new(releases, cx));
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
            view
        },
    )
}

async fn capture(
    cx: &mut AsyncApp,
    scene: Scene,
    output: &Path,
    releases: Arc<[Release]>,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let view = cx.update(|cx| open(cx, scene, releases, false))?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    sound::activate_accessibility(raw, cx)?;
    draw(raw, scene, Some(&output.join("library.png")), cx)?;
    press(raw, "concept-release-0", cx)?;
    clear_focus(view, cx)?;
    draw(raw, scene, Some(&output.join("album.png")), cx)?;
    let (album_rows, remaining) = view.update(cx, |view, _, _| {
        (
            view.releases[0].info.tracks.len(),
            remaining_track_scroll(&view.track_scroll),
        )
    })?;
    let mut fade_check = None;
    if remaining > px(1.) {
        let position = view.update(cx, |view, _, _| view.track_scroll.bounds().center())?;
        raw.update(cx, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), -px(48.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })?;
        draw(raw, scene, None, cx)?;
        let after_wheel =
            view.update(cx, |view, _, _| remaining_track_scroll(&view.track_scroll))?;
        if after_wheel >= remaining {
            return Err(io::Error::other("Колесо не прокрутило трек-лист.").into());
        }
        let last = album_rows - 1;
        press(raw, &format!("concept-row-{last}"), cx)?;
        clear_focus(view, cx)?;
        draw(raw, scene, Some(&output.join("album-bottom.png")), cx)?;
        let (remaining_at_end, last_visible) = view.update(cx, |view, _, _| {
            let scroll = &view.track_scroll;
            let last_visible = scroll.bounds_for_item(last).is_some_and(|row| {
                (row.bottom() + scroll.offset().y) <= scroll.bounds().bottom() + px(1.)
                    && view.browse
                        == Browse::Release(TrackId {
                            release: 0,
                            index: last,
                        })
            });
            (remaining_track_scroll(scroll), last_visible)
        })?;
        if !last_visible || remaining_at_end > px(1.) {
            return Err(io::Error::other(format!(
                "Последний трек не дошёл до края списка: {remaining_at_end:?}, {last_visible}"
            ))
            .into());
        }
        fade_check = Some(json!({
            "rows": album_rows,
            "remaining_at_top_dip": f32::from(remaining),
            "remaining_after_wheel_dip": f32::from(after_wheel),
            "remaining_at_bottom_dip": f32::from(remaining_at_end),
            "last_row_fully_visible_with_keyboard": last_visible,
        }));
    }
    press(raw, "concept-search", cx)?;
    draw(raw, scene, None, cx)?;
    for stroke in ["b", "o", "o", "k"] {
        raw.update(cx, |_, window, cx| type_key(window, stroke, cx))??;
    }
    draw(raw, scene, Some(&output.join("search.png")), cx)?;
    let search = sound::accessibility(raw, &output.join("search-a11y.json"), cx)?;
    if sound::node(&search, "concept-release-0").is_some()
        || sound::node(&search, "concept-release-1").is_none()
    {
        return Err(io::Error::other("Поиск не отфильтровал релизы по исполнителю.").into());
    }
    press(raw, "concept-release-1", cx)?;
    press(raw, "concept-row-1", cx)?;
    clear_focus(view, cx)?;
    draw(raw, scene, Some(&output.join("release.png")), cx)?;
    press(raw, "concept-add-next", cx)?;
    press(raw, "concept-queue", cx)?;
    clear_focus(view, cx)?;
    draw(raw, scene, Some(&output.join("queue.png")), cx)?;
    let tree = sound::accessibility(raw, &output.join("queue-a11y.json"), cx)?;
    if sound::node_label(&tree, "concept-queue-title") != Some("Очередь") {
        return Err(io::Error::other("Очередь не появилась в дереве GPUI.").into());
    }
    let queued = view.update(cx, |view, _, _| {
        let wanted = TrackId {
            release: 1,
            index: 1,
        };
        view.queue_open
            && view.following.first() == Some(&wanted)
            && view.current
                == (TrackId {
                    release: 0,
                    index: 0,
                })
    })?;
    press(raw, "concept-library", cx)?;
    let restored = view.update(cx, |view, _, _| {
        view.browse
            == Browse::Release(TrackId {
                release: 1,
                index: 1,
            })
            && !view.queue_open
    })?;
    press(raw, "concept-back", cx)?;
    let search_restored = view.update(cx, |view, _, cx| {
        view.browse == Browse::Search && view.query.read(cx).text() == "book"
    })?;
    let position_goal = view.update(cx, |view, _, _| {
        (view.position_seconds + 5).min(view.duration_seconds())
    })?;
    focus_control(raw, "concept-seek", cx)?;
    raw.update(cx, |_, window, cx| type_key(window, "right", cx))??;
    focus_control(raw, "concept-volume", cx)?;
    raw.update(cx, |_, window, cx| type_key(window, "left", cx))??;
    let transport_adjusted = view.update(cx, |view, _, _| {
        view.position_seconds == position_goal && view.volume_percent == 67
    })?;
    press(raw, "concept-next", cx)?;
    let advanced = view.update(cx, |view, _, _| {
        view.current
            == (TrackId {
                release: 1,
                index: 1,
            })
            && view.following.len() == 2
    })?;
    press(raw, "concept-prev", cx)?;
    let previous_restored = view.update(cx, |view, _, _| {
        view.current
            == (TrackId {
                release: 0,
                index: 0,
            })
            && view.following.first()
                == Some(&TrackId {
                    release: 1,
                    index: 1,
                })
    })?;
    if !queued
        || !restored
        || !search_restored
        || !advanced
        || !transport_adjusted
        || !previous_restored
    {
        return Err(io::Error::other("Не прошли очередь, возврат или команды плеера.").into());
    }
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "entry": "library",
            "font_family": layout::FONT,
            "search_filters_by_artist": true,
            "next_track_added_without_changing_current": queued,
            "release_and_selection_restored": restored,
            "search_query_restored": search_restored,
            "next_command_started_queued_track": advanced,
            "seek_and_volume_by_arrow_keys": transport_adjusted,
            "previous_command_restored_track": previous_restored,
            "track_list_fade": fade_check,
            "input": "GPUI Tab, Enter and arrow keys",
        }))?,
    )?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn type_key(window: &mut gpui::Window, stroke: &str, cx: &mut gpui::App) -> gpui::Result<()> {
    let keystroke = Keystroke::parse(stroke)?;
    window.dispatch_keystroke(keystroke.clone(), cx);
    window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
    Ok(())
}

fn clear_focus(view: WindowHandle<Concept>, cx: &mut AsyncApp) -> gpui::Result<()> {
    view.update(cx, |view, window, cx| window.focus(&view.focus, cx))
}

fn press(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    focus_control(raw, id, cx)?;
    raw.update(cx, |_, window, cx| type_key(window, "enter", cx))?
}

fn focus_control(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let element = format!("Name({id:?})");
    raw.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        for _ in 0..=64 {
            let snapshot: Value = serde_json::from_str(
                &window
                    .debug_a11y_tree_json()
                    .ok_or_else(|| io::Error::other("Нет дерева доступности GPUI."))?,
            )?;
            let focused = snapshot["gpui_focus"].as_str().unwrap_or_default();
            if snapshot["nodes"][focused]["element_id"] == element {
                return Ok(());
            }
            type_key(window, "tab", cx)?;
            window.draw(cx).clear(cx);
        }
        Err(io::Error::other(format!("Фокус не достиг {id}.")).into())
    })?
}
