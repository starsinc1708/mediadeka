#[cfg(feature = "capture")]
mod acceptance;
mod content;
mod feeds;
mod fluid;
mod for_you;
mod listening;
mod mix;
mod shelf_motion;
mod skeleton;
mod sources;

use std::{cell::Cell, collections::BTreeMap, rc::Rc};

use gpui::{AnyElement, App, Context, FocusHandle, Window, div, prelude::*, px};

use crate::{
    music::ObjectRef,
    providers::{HomeItem, ProviderId},
    state::HomeContent,
};

use super::{
    MainView,
    components::{Activate, artwork, button},
    design_system::adaptive_layout::{AdaptiveContent, LayoutBand},
    workspace::{Page, WorkspaceLayout},
};
use feeds::{Feed, FeedSource, Feeds};

pub(super) const SOUNDCLOUD_ACTIVITY: &str = "Лента подписок";

#[derive(Clone, Copy)]
struct Layout {
    glass: Option<super::components::glass_control::Material>,
    compact: bool,
    short: bool,
    content_width: f32,
    columns: usize,
    card_width: f32,
    quick_columns: usize,
    quick_width: f32,
}

impl Layout {
    fn new(
        workspace: WorkspaceLayout,
        height: gpui::Pixels,
        adaptive: Option<AdaptiveContent>,
    ) -> Self {
        let WorkspaceLayout {
            compact,
            content_width,
            width,
            ..
        } = workspace;
        let content_width = adaptive.map_or(content_width, |layout| layout.width);
        let compact = adaptive.map_or(compact, AdaptiveContent::compact);
        let short = compact && height < px(500.);
        let columns = if let Some(layout) = adaptive {
            match layout.band {
                LayoutBand::Narrow => 3,
                LayoutBand::Compact if short => 5,
                LayoutBand::Compact => 4,
                LayoutBand::Single => 5,
                LayoutBand::Standard => 6,
                LayoutBand::Expanded => 7,
            }
        } else if short {
            5
        } else if width >= 1470. {
            7
        } else if compact {
            4
        } else {
            6
        };
        let quick_columns = if compact { 3 } else { 4 };
        let cell_width = |count| (content_width - 12. * (count - 1) as f32) / count as f32;
        Self {
            glass: None,
            compact,
            short,
            content_width,
            columns,
            card_width: cell_width(columns).min(if short { 112. } else { 186. }),
            quick_columns,
            quick_width: cell_width(quick_columns),
        }
    }

    fn with_glass(mut self, glass: Option<super::components::glass_control::Material>) -> Self {
        if glass.is_some() {
            self.quick_width = self.quick_width.min(252.);
        }
        self.glass = glass;
        self
    }

    fn section_gap(self) -> f32 {
        if self.glass.is_some() {
            return if self.short { 16. } else { 32. };
        }
        if self.short {
            9.
        } else if self.compact {
            18.
        } else {
            27.
        }
    }

    fn continuation_rail(self) -> bool {
        self.glass.is_some() && !self.short && self.content_width >= 1050.
    }
}

pub(super) struct HomeUi {
    shelf_pages: BTreeMap<(ProviderId, usize), usize>,
    shelf_turns: BTreeMap<(ProviderId, usize), shelf_motion::ShelfTurn>,
    shelf_tabs: BTreeMap<(ProviderId, usize), usize>,
    sources_open: bool,
    filter: Option<ProviderId>,
    feeds: Feeds,
    resume: Option<gpui::Task<()>>,
    mix: mix::MixUi,
    shelf_origins: [Rc<Cell<f32>>; 2],
    queue_focus: FocusHandle,
    #[cfg(feature = "capture")]
    for_you_origin: Rc<Cell<f32>>,
}

#[derive(Clone, Copy)]
struct FeedContext<'a> {
    provider: ProviderId,
    profile: Option<&'a str>,
    combined: bool,
}

impl<'a> FeedContext<'a> {
    fn new(source: &'a FeedSource, combined: bool) -> Self {
        Self {
            provider: source.provider,
            profile: source.profile.as_deref(),
            combined,
        }
    }

    fn id(self, prefix: &str, index: usize) -> String {
        if self.combined {
            format!("{prefix}-{}-{index}", self.provider.as_str())
        } else {
            format!("{prefix}-{index}")
        }
    }

    fn key(self) -> FeedSource {
        FeedSource {
            provider: self.provider,
            profile: self.profile.map(str::to_owned),
        }
    }
}

impl HomeUi {
    pub(super) fn new(cx: &App) -> Self {
        Self {
            shelf_pages: BTreeMap::new(),
            shelf_turns: BTreeMap::new(),
            shelf_tabs: BTreeMap::new(),
            sources_open: false,
            filter: None,
            feeds: Feeds::default(),
            resume: None,
            mix: mix::MixUi::default(),
            shelf_origins: Default::default(),
            queue_focus: cx
                .focus_handle()
                .tab_index(for_you::QUEUE_ORDER)
                .tab_stop(true),
            #[cfg(feature = "capture")]
            for_you_origin: Default::default(),
        }
    }

    pub(super) fn prepare_page(&mut self, page: Page) {
        self.shelf_turns.clear();
        if page == Page::Home {
            self.shelf_pages.clear();
            self.shelf_tabs.clear();
        } else {
            self.sources_open = false;
            self.resume = None;
            self.mix.clear();
        }
    }
}

impl MainView {
    fn track_home_section_origin(&self, position: Rc<Cell<f32>>) -> impl IntoElement {
        let scroll = self.scroll.clone();
        gpui::canvas(
            |_, _, _| (),
            move |bounds, (), _, _| {
                position.set(f32::from(
                    bounds.top() - scroll.bounds().top() - scroll.offset().y,
                ));
            },
        )
        .absolute()
        .top_0()
        .left_0()
    }

    pub(super) fn home_feed(
        &self,
        home: Option<&crate::state::HomeView>,
        workspace: WorkspaceLayout,
        window: &Window,
        refreshing: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let glass = self.glass_controls(cx);
        let adaptive = glass.map(|_| self.adaptive_content(window));
        let layout =
            Layout::new(workspace, window.viewport_size().height, adaptive).with_glass(glass);
        let Some(home) = home else {
            let pending = skeleton::loading(
                layout,
                None,
                (!self.reduced_motion).then(|| self.presentation_time()),
            );
            return if layout.glass.is_some() {
                div()
                    .child(fluid::heading(layout).mb(px(layout.section_gap())))
                    .child(pending)
                    .into_any_element()
            } else {
                pending.into_any_element()
            };
        };
        let active = FeedSource::active(home);
        let sources = FeedSource::available(home);
        let visible: Vec<_> = sources
            .iter()
            .filter(|source| {
                self.home
                    .filter
                    .is_none_or(|current| current == source.provider)
            })
            .collect();
        let listening::Listening {
            spotlight,
            continuation,
            sections,
        } = self.home_listening(&visible, layout, cx);
        let for_you = self.home_for_you(&visible, &active, home, refreshing, layout, cx);
        let content = if visible.len() == 1
            && visible[0] == &active
            && matches!(home.content, HomeContent::Loading)
        {
            skeleton::loading(
                layout,
                Some(active.provider),
                (!self.reduced_motion).then(|| self.presentation_time()),
            )
            .into_any_element()
        } else if let [source] = visible.as_slice() {
            let feed = self.home.feeds.get(source, &active, home, refreshing);
            self.music_content(home, FeedContext::new(source, false), feed, layout, cx)
                .into_any_element()
        } else {
            let feeds = visible
                .into_iter()
                .map(|source| {
                    (
                        FeedContext::new(source, true),
                        self.home.feeds.get(source, &active, home, refreshing),
                    )
                })
                .collect();
            self.combined_music_content(home, feeds, layout, cx)
                .into_any_element()
        };
        let heading = glass.map(|_| fluid::heading(layout));
        let header = self.source_row(home, &active, &sources, refreshing, heading, cx);
        let show_intro = layout.glass.is_some() && spotlight.is_none();
        let has_listening = spotlight.is_some() || continuation.is_some();
        let listening =
            if layout.continuation_rail() && spotlight.is_some() && continuation.is_some() {
                div()
                    .flex()
                    .items_start()
                    .gap(px(crate::ui::fluid_glass::tokens::space::XL))
                    .child(div().flex_1().min_w_0().children(spotlight))
                    .child(
                        div()
                            .w(px(layout.quick_width))
                            .flex_none()
                            .children(continuation),
                    )
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(layout.section_gap()))
                    .children(spotlight)
                    .children(continuation)
            };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(layout.section_gap()))
            .child(header)
            .when(has_listening, |page| page.child(listening))
            .when(show_intro, |page| {
                page.child(self.home_library_intro(layout, cx))
            })
            .children(sections)
            .children(for_you)
            .child(content)
            .into_any_element()
    }
}

fn open_track(
    source: &FeedSource,
    item: HomeItem,
    view: &mut MainView,
    window: &Window,
    cx: &mut Context<MainView>,
) {
    if let Some((home, _)) = view.state.home_view()
        && FeedSource::available(home).contains(source)
    {
        if item.id == "spotify:collection:tracks" || item.id == "browse:VLLM" {
            view.show_music_page(Page::Tracks, window, cx);
        } else {
            view.open_music_item(source.provider, source.profile.as_deref(), item, window, cx);
        }
    }
}

fn openable(provider: ProviderId, item: &HomeItem) -> bool {
    ObjectRef::from_item(provider, item).is_some()
        || matches!(
            item.id.as_str(),
            "spotify:collection:tracks" | "browse:VLLM"
        )
}
