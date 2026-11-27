use std::{fs, io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, Keystroke, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    SAVED_RESULT, SEARCH_QUERY, Scene, TITLE, by_element, dispatch, focused, key, labeled_button,
    named, node_label, press, queue_candidate, require, snapshot, wait,
};

const SEARCH_INPUT: &str = "home-search";
const SEARCH_HEADING: &str = "music-search-title";
const LIBRARY_HEADING: &str = "music-liked-title";
const LIBRARY_BUTTON: &str = "studio-library";
const PLAYER_SEEK: &str = "player-progress";
const SETTLED_MS: u64 = 220;
const NEXT_TITLE: &str = "Северный ветер";
type Region = [u32; 4];
type Pixel = [u8; 4];

#[derive(Clone, Copy)]
struct Probes {
    heading: Region,
    heading_before_search: Region,
    search_icon: Region,
    player: Region,
}

const WIDE: Probes = Probes {
    heading: [303, 94, 518, 159],
    heading_before_search: [303, 94, 380, 159],
    search_icon: [396, 34, 415, 52],
    player: [931, 973, 989, 1026],
};
const COMPACT: Probes = Probes {
    heading: [93, 64, 226, 110],
    heading_before_search: [93, 64, 160, 110],
    search_icon: [174, 22, 193, 43],
    player: [378, 332, 420, 371],
};

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let probes = if scene.reduced { COMPACT } else { WIDE };
    let capture = Capture {
        window,
        scene,
        output,
        probes,
    };
    let result = capture.exercise(cx).await;
    capture.set_time(None, cx)?;
    result
}

struct Capture<'a> {
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &'a Path,
    probes: Probes,
}

impl Capture<'_> {
    fn raw(&self) -> AnyWindowHandle {
        self.window.into()
    }

    async fn exercise(&self, cx: &mut AsyncApp) -> gpui::Result<()> {
        let raw = self.raw();
        self.set_time(Some(SETTLED_MS), cx)?;
        let initial = self.read(None, cx)?;
        require(
            initial.library_visible,
            "Проверка смены страницы должна начаться в медиатеке.",
        )?;
        key(raw, "ctrl-k", cx)?;
        self.set_time(Some(0), cx)?;
        let query_focused = self.read(None, cx)?;
        require(
            query_focused.library_visible
                && query_focused.focused_on(SEARCH_INPUT)
                && initial.heading_before_search.is_some()
                && query_focused.heading_before_search == initial.heading_before_search,
            "Ctrl+K должен оставить медиатеку неподвижной и сфокусировать поле поиска.",
        )?;

        enter_query(raw, SEARCH_QUERY, cx)?;
        key(raw, "enter", cx)?;
        await_match(raw, TITLE, cx).await?;
        let search_start = self.read(Some("page-motion-search-start.png"), cx)?;
        self.set_time(Some(80), cx)?;
        let search_middle = self.read(Some("page-motion-search-mid.png"), cx)?;
        self.set_time(Some(SETTLED_MS), cx)?;
        let search_end = self.read(Some("page-motion-search-settled.png"), cx)?;

        self.set_time(Some(0), cx)?;
        enter_query(raw, "северный ветер", cx)?;
        key(raw, "enter", cx)?;
        key(raw, "enter", cx)?;
        await_match(raw, NEXT_TITLE, cx).await?;
        let search_edited = self.read(Some("page-motion-search-edited.png"), cx)?;

        press(raw, &named(LIBRARY_BUTTON), cx)?;
        wait(
            cx,
            "строки медиатеки после возврата",
            |cx| Ok(queue_candidate(&snapshot(raw, cx)?).is_some().then_some(())),
        )
        .await?;
        let library_start = self.read(Some("page-motion-library-start.png"), cx)?;
        self.set_time(Some(SETTLED_MS), cx)?;
        let library_end = self.read(Some("page-motion-library-settled.png"), cx)?;

        let [search_offset, search_middle_offset, library_offset] = [
            search_start.top() - search_end.top(),
            search_middle.top() - search_end.top(),
            library_start.top() - library_end.top(),
        ];
        let search_frames = [&search_start, &search_middle, &search_end, &search_edited];
        let toolbar_differences =
            search_frames.map(|obs| changes(&search_start.toolbar, &obs.toolbar));
        let player_differences = [
            &search_start,
            &search_middle,
            &search_end,
            &search_edited,
            &library_start,
            &library_end,
        ]
        .map(|obs| changes(&query_focused.player, &obs.player));
        let report = json!({
            "reduced_motion": self.scene.reduced,
            "fixed_pixels_exclusive_right_bottom": {
                "toolbar_search_icon": self.probes.search_icon,
                "player": self.probes.player,
                "workspace_heading": self.probes.heading,
            },
            "search_start_offset_px": search_offset,
            "search_80ms_offset_px": search_middle_offset,
            "library_start_offset_px": library_offset,
            "changed_toolbar_pixels_from_search_start": toolbar_differences,
            "changed_player_pixels_from_focused_query": player_differences,
            "initial_library": initial.summary(),
            "ctrl_k": query_focused.summary(),
            "search_start": search_start.summary(),
            "search_80ms": search_middle.summary(),
            "search_settled": search_end.summary(),
            "search_repeated": search_edited.summary(),
            "library_start": library_start.summary(),
            "library_settled": library_end.summary(),
        });
        fs::write(
            self.output.join("page-motion.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!(
            "{} page motion: Search start/middle {search_offset}/{search_middle_offset}px, Library start {library_offset}px, toolbar changes {toolbar_differences:?}, player {player_differences:?}",
            self.scene.name,
        );
        let appropriate_motion = if self.scene.reduced {
            [search_offset, search_middle_offset, library_offset] == [0; 3]
        } else {
            search_offset == 6
                && 0 < search_middle_offset
                && search_middle_offset < search_offset
                && library_offset == 6
        };
        require(
            appropriate_motion
                && search_frames
                    .iter()
                    .all(|sample| sample.search_visible && sample.focused_on(SEARCH_INPUT))
                && search_end.heading == search_edited.heading
                && library_start.library_visible
                && library_end.library_visible
                && library_end.focused_on(LIBRARY_BUTTON)
                && library_end.heading == initial.heading
                && toolbar_differences == [0; 4]
                && player_differences == [0; 6],
            "Переходы должны сдвигать рабочую область, сохраняя фокус и положение поисковой строки и плеера.",
        )
    }

    fn set_time(&self, elapsed_ms: Option<u64>, cx: &mut AsyncApp) -> gpui::Result<()> {
        self.window.update(cx, |view, _, cx| {
            view.set_fluid_glass_page_motion_for_capture(elapsed_ms.map(Duration::from_millis), cx);
        })
    }

    fn read(&self, artifact: Option<&str>, cx: &mut AsyncApp) -> gpui::Result<Observation> {
        let tree = snapshot(self.raw(), cx)?;
        let image = render_image(self.raw(), self.scene, cx)?;
        if let Some(name) = artifact {
            image.save(self.output.join(name))?;
        }
        let heading = glyph_bounds(&image, self.probes.heading).ok_or_else(|| {
            io::Error::other("Заголовок рабочей области отсутствует на нативном кадре.")
        })?;
        require(
            node_label(&tree, PLAYER_SEEK) == Some("Позиция воспроизведения"),
            "Перемотка плеера должна оставаться доступной в процессе смены страницы.",
        )?;
        Ok(Observation {
            heading,
            // Recent queries cover the right part of the heading when Ctrl+K
            // focuses the empty input. Its unobscured prefix still proves that
            // the underlying page did not move.
            heading_before_search: glyph_bounds(&image, self.probes.heading_before_search),
            toolbar: crop(&image, self.probes.search_icon),
            player: crop(&image, self.probes.player),
            focus: focused(&tree).map(str::to_owned),
            library_visible: node_label(&tree, LIBRARY_HEADING) == Some("Медиатека")
                && queue_candidate(&tree).is_some(),
            search_visible: node_label(&tree, SEARCH_HEADING) == Some("Поиск"),
            result_label: by_element(&tree, SAVED_RESULT)
                .and_then(|node| node["aria"]["label"].as_str())
                .map(str::to_owned),
        })
    }
}

struct Observation {
    heading: Region,
    heading_before_search: Option<Region>,
    toolbar: Vec<Pixel>,
    player: Vec<Pixel>,
    focus: Option<String>,
    library_visible: bool,
    search_visible: bool,
    result_label: Option<String>,
}

impl Observation {
    fn focused_on(&self, id: &str) -> bool {
        self.focus.as_deref() == Some(named(id).as_str())
    }

    fn top(&self) -> i32 {
        self.heading[1] as i32
    }

    fn summary(&self) -> Value {
        json!({
            "heading_bright_pixels_inclusive": self.heading,
            "heading_before_search": self.heading_before_search,
            "focus": self.focus,
            "library_visible": self.library_visible,
            "search_visible": self.search_visible,
            "saved_result": self.result_label,
        })
    }
}

fn enter_query(raw: AnyWindowHandle, query: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    key(raw, "ctrl-a", cx)?;
    for character in query.chars() {
        let text = character.to_string();
        dispatch(
            raw,
            Keystroke {
                key: text.clone(),
                key_char: Some(text),
                ..Default::default()
            },
            cx,
        )?;
    }
    Ok(())
}

async fn await_match(raw: AnyWindowHandle, title: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    wait(cx, title, |cx| {
        let tree = snapshot(raw, cx)?;
        Ok(by_element(&tree, SAVED_RESULT)
            .is_some_and(|row| labeled_button(row, title))
            .then_some(()))
    })
    .await
}

fn glyph_bounds(image: &image::RgbaImage, [left, top, right, bottom]: Region) -> Option<Region> {
    let mut bounds = None;
    for (x, y) in (top..bottom).flat_map(|y| (left..right).map(move |x| (x, y))) {
        let [r, g, b, _] = image.get_pixel(x, y).0;
        if r.min(g).min(b) > 180 {
            let [min_x, min_y, max_x, max_y] = bounds.unwrap_or([x, y, x, y]);
            bounds = Some([min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]);
        }
    }
    bounds
}

fn crop(image: &image::RgbaImage, [left, top, right, bottom]: Region) -> Vec<Pixel> {
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| image.get_pixel(x, y).0))
        .collect()
}

fn changes(left: &[Pixel], right: &[Pixel]) -> usize {
    left.iter().zip(right).filter(|(a, b)| a != b).count()
}
