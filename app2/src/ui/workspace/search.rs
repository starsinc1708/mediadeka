//! The global catalog field in the workspace toolbar.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, KeyDownEvent, Subscription, Window,
};
use guise::{Size, TextInput, TextInputEvent};

use crate::{
    music::{ObjectKind, SearchInput},
    ui::components::search_input,
};

use super::MainView;
use super::routes::Route;

const HISTORY_LIMIT: usize = 12;
const SUGGESTION_LIMIT: usize = 4;

pub(in crate::ui) struct SearchBar {
    field: Entity<TextInput>,
    previous_focus: Option<FocusHandle>,
    pub(super) suggestions_focus: FocusHandle,
    history: Vec<String>,
    pub(super) highlighted: Option<usize>,
    hide_suggestions: bool,
    _submit_subscription: Subscription,
}

impl SearchBar {
    pub(super) fn new(window: &Window, fluid: bool, cx: &mut Context<MainView>) -> Self {
        let field = cx.new(|cx| {
            search_input::editor(
                "home-search",
                if fluid {
                    "Поиск по всей музыке"
                } else {
                    "Название или ссылка"
                },
                10,
                Size::Xs,
                fluid,
                cx,
            )
            .accessible_name("Поиск музыки")
            .short_placeholder("Поиск")
        });
        let submit_subscription =
            cx.subscribe_in(&field, window, |view, _, event, window, cx| match event {
                TextInputEvent::Submit(_) => view.submit_toolbar_search(window, cx),
                TextInputEvent::Change(_) if !view.workspace.search.history.is_empty() => {
                    let search = &mut view.workspace.search;
                    search.highlighted = None;
                    search.hide_suggestions = false;
                    cx.notify();
                }
                TextInputEvent::Change(_) => {}
            });
        Self {
            field,
            previous_focus: None,
            suggestions_focus: cx.focus_handle(),
            history: Vec::new(),
            highlighted: None,
            hide_suggestions: false,
            _submit_subscription: submit_subscription,
        }
    }

    pub(super) fn field(&self) -> Entity<TextInput> {
        self.field.clone()
    }

    pub(in crate::ui) fn text(&self, cx: &App) -> String {
        self.field.read(cx).text()
    }

    /// Show the query of the current `Search` route. Other routes keep the
    /// field empty, so typed text never changes the route by itself.
    pub(super) fn show_route(&mut self, route: &Route, cx: &mut Context<MainView>) {
        let query = match route {
            Route::Search { query, .. } => query.as_str(),
            _ => "",
        };
        self.highlighted = None;
        self.hide_suggestions = !query.is_empty();
        self.field.update(cx, |field, cx| {
            if field.text() != query {
                field.set_text(query, cx);
            }
        });
    }

    pub(in crate::ui) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.field.read(cx).is_focused(window)
    }

    pub(super) fn focus(&mut self, window: &mut Window, cx: &mut Context<MainView>) {
        // A repeated Ctrl+K keeps the focus that the first one interrupted.
        if !self.is_focused(window, cx) {
            self.previous_focus = window.focused(cx);
        }
        self.hide_suggestions = false;
        self.field.update(cx, |field, cx| field.focus(window, cx));
        cx.notify();
    }

    /// Esc in the field returns the route query and the focus that Ctrl+K
    /// interrupted, or the window root when there was no previous focus.
    pub(in crate::ui) fn dismiss(
        &mut self,
        route_query: &str,
        root: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<MainView>,
    ) {
        self.highlighted = None;
        self.hide_suggestions = true;
        self.field
            .update(cx, |field, cx| field.set_text(route_query, cx));
        match self.previous_focus.take() {
            Some(previous) => window.focus(&previous, cx),
            None => window.focus(root, cx),
        }
        cx.notify();
    }

    pub(super) fn suggestions<'a>(&'a self, window: &Window, cx: &App) -> Vec<&'a str> {
        if !(self.is_focused(window, cx) || self.suggestions_focus.contains_focused(window, cx)) {
            return Vec::new();
        }
        let text = self.text(cx);
        if self.hide_suggestions && !text.trim().is_empty() {
            return Vec::new();
        }
        let prefix = text.trim().to_lowercase();
        self.history
            .iter()
            .filter(|query| {
                let query = query.to_lowercase();
                query != prefix && query.starts_with(&prefix)
            })
            .take(SUGGESTION_LIMIT)
            .map(String::as_str)
            .collect()
    }

    fn record_query(&mut self, cx: &App) {
        let text = self.text(cx);
        if let Some(SearchInput::Text { query, .. }) = SearchInput::new(ObjectKind::Track, &text) {
            let normalized = query.to_lowercase();
            self.history
                .retain(|previous| previous.to_lowercase() != normalized);
            self.history.insert(0, query);
            self.history.truncate(HISTORY_LIMIT);
        }
        self.highlighted = None;
        self.hide_suggestions = true;
    }
}

impl MainView {
    pub(super) fn submit_toolbar_search(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.uses_fluid_glass() {
            self.workspace.search.record_query(cx);
        }
        self.submit_global_search(window, cx);
        cx.notify();
    }

    pub(super) fn choose_search_suggestion(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.search.field.update(cx, |field, cx| {
            field.set_text(query, cx);
            field.focus(window, cx);
        });
        self.submit_toolbar_search(window, cx);
    }

    pub(super) fn on_search_suggestion_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let search = &mut self.workspace.search;
        let suggestions = search.suggestions(window, cx);
        let count = suggestions.len();
        if count == 0 {
            return;
        }
        let current = search.highlighted.filter(|index| *index < count);
        match event.keystroke.key.as_str() {
            "down" => search.highlighted = Some(current.map_or(0, |index| (index + 1) % count)),
            "up" => {
                search.highlighted =
                    Some(current.map_or(count - 1, |index| (index + count - 1) % count))
            }
            "enter" => {
                let Some(index) = current else {
                    return;
                };
                let query = suggestions[index].to_owned();
                self.choose_search_suggestion(&query, window, cx);
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
}
