mod panel;
#[cfg(all(test, windows, feature = "capture"))]
mod tests;

use std::{cell::Cell, rc::Rc};

use gpui::{
    App, AppContext, Context, Div, Entity, FontWeight, IntoElement, Role, ScrollHandle, Window,
    div, prelude::*, px,
};
use guise::{Button, Icon, IconName, Size, TextInput, Variant, theme};

use crate::{
    providers::{
        self, ErrorKind, LoginInput, LoginPrompt, ProviderError, ProviderId, YouTubeChoices,
    },
    state::{Failure, Intent, Issue, Request, Screen},
    storage::Store,
    telemetry::{self, CatalogInvalidation},
};

use super::{
    MainView,
    assets::{BRAND_MARKS, mark, provider_index},
    components::{
        Activate, button,
        glass_control::{Material, Tone},
        heading,
    },
    design_system::controls::{ControlState, ControlVariant},
    theme::{icon_button_hover, primary},
};

fn soundcloud_input(window: &Window) -> LoginInput {
    LoginInput::SoundCloudBrowser {
        owner: super::native::login_owner(window),
    }
}

fn youtube_validation_error(screen: &Screen) -> Option<&ProviderError> {
    let Screen::Failed(Failure {
        intent: Intent::SignIn(ProviderId::YouTubeMusic) | Intent::Connect(ProviderId::YouTubeMusic),
        issue: Issue::Provider(error),
    }) = screen
    else {
        return None;
    };
    matches!(error.kind, ErrorKind::InvalidInput | ErrorKind::Expired).then_some(error)
}

pub(super) struct Connection {
    provider: ProviderId,
    form: Option<Form>,
    phase: Phase,
    scroll: ScrollHandle,
    revealed_field: Rc<Cell<Option<(usize, usize)>>>,
}

impl Connection {
    fn new(provider: ProviderId, cx: &mut Context<MainView>) -> Self {
        let form = match provider {
            ProviderId::YouTubeMusic => Some(Form::YouTube {
                session: cx.new(|cx| {
                    login_field("youtube-session", "Данные входа (Cookie)", 21, cx)
                        .placeholder("Вставьте значение Cookie")
                        .password(true)
                }),
                help_open: false,
                missing: false,
            }),
            ProviderId::Jellyfin => Some(Form::Jellyfin {
                server: cx.new(|cx| {
                    login_field("jellyfin-server", "Адрес сервера Jellyfin", 20, cx)
                        .placeholder("Например, https://music.example.org")
                }),
                username: cx.new(|cx| login_field("jellyfin-username", "Имя пользователя", 21, cx)),
                password: cx
                    .new(|cx| login_field("jellyfin-password", "Пароль", 22, cx).password(true)),
            }),
            ProviderId::Spotify | ProviderId::YandexMusic | ProviderId::SoundCloud => None,
        };
        Self {
            provider,
            form,
            phase: Phase::Setup,
            scroll: ScrollHandle::new(),
            revealed_field: Rc::new(Cell::new(None)),
        }
    }

    pub(super) fn provider(&self) -> ProviderId {
        self.provider
    }

    fn first_field(&self) -> Option<&Entity<TextInput>> {
        match self.form.as_ref()? {
            Form::YouTube { session, .. } => Some(session),
            Form::Jellyfin { server, .. } => Some(server),
        }
    }
}

fn login_field(
    id: &'static str,
    label: &'static str,
    order: isize,
    cx: &mut Context<TextInput>,
) -> TextInput {
    TextInput::new(cx)
        .id(id)
        .label(label)
        .accessible_name(label)
        .size(Size::Sm)
        .idle_border(theme(cx).dimmed().alpha(0.70))
        .tab_order(order)
}

enum Phase {
    Setup,
    Working(Option<LoginPrompt>),
    ChooseYouTube {
        request: Request,
        choices: YouTubeChoices,
    },
    Finished,
}

enum Form {
    YouTube {
        session: Entity<TextInput>,
        help_open: bool,
        missing: bool,
    },
    Jellyfin {
        server: Entity<TextInput>,
        username: Entity<TextInput>,
        password: Entity<TextInput>,
    },
}

enum Outcome {
    Prompt(LoginPrompt),
    Profiles(YouTubeChoices),
    Done(Screen),
}

impl MainView {
    fn submit_youtube(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Form::YouTube {
            session, missing, ..
        }) = self.login.as_mut().and_then(|login| login.form.as_mut())
        else {
            return;
        };
        let cookies = session.read(cx).text();
        *missing = cookies.trim().is_empty();
        if *missing {
            session.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        } else {
            self.submit_connection(LoginInput::YouTubeMusic { cookies }, window, cx);
        }
    }

    pub(super) fn select_provider(
        &mut self,
        provider: ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .login
            .as_ref()
            .is_some_and(|login| login.provider == provider)
        {
            self.focus_connection(window, cx);
            return;
        }
        if !self.state.can_leave_login() {
            return;
        }
        self.state.reset();
        self.reset_scroll();
        self.open_connection_motion(provider, window);
        self.start_connection(provider, window, cx);
    }

    pub(super) fn is_settings_login(&self) -> bool {
        self.workspace.on_settings() && self.login.is_some()
    }

    #[cfg(feature = "capture")]
    pub fn is_settings_login_for_capture(&self) -> bool {
        self.is_settings_login()
    }

    #[cfg(feature = "capture")]
    pub fn has_settings_provider_focus_for_capture(
        &self,
        provider: ProviderId,
        window: &Window,
    ) -> bool {
        self.provider_focus[provider_index(provider)].is_focused(window)
    }

    #[cfg(feature = "capture")]
    pub fn active_profile_for_capture(&self) -> Option<(ProviderId, String)> {
        use crate::state::ActiveSession;

        match &self.state.home_view()?.0.session {
            ActiveSession::Guest => None,
            ActiveSession::Account {
                provider, profile, ..
            } => Some((*provider, profile.id.clone())),
        }
    }

    #[cfg(feature = "capture")]
    pub fn open_settings_login_for_capture(
        &mut self,
        provider: ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_connection(provider, window, cx);
    }

    pub(super) fn open_settings_connection(
        &mut self,
        provider: ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace.on_settings()
            || self.is_settings_login()
            || !self.state.home_view().is_some_and(|(_, busy)| !busy)
        {
            return;
        }
        self.stop_workspace_scrolling();
        self.connection_motion.clear_flight();
        self.start_connection(provider, window, cx);
    }

    fn start_connection(
        &mut self,
        provider: ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.login = Some(Connection::new(provider, cx));
        self.focus_connection(window, cx);
        match provider {
            ProviderId::Spotify => self.submit_connection(LoginInput::Spotify, window, cx),
            ProviderId::YandexMusic => self.submit_connection(LoginInput::YandexDevice, window, cx),
            _ => cx.notify(),
        }
    }

    pub(super) fn focus_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .connection_motion
            .at(self.presentation_time(), self.reduced_motion)
            .flight
            .is_some_and(|flight| !flight.controls_visible())
        {
            self.connection_focus_pending = true;
            window.focus(&self.connection_focus, cx);
            return;
        }
        if let Some(field) = self.login.as_ref().and_then(Connection::first_field) {
            field.update(cx, |field, cx| field.focus(window, cx));
        } else {
            window.focus(&self.connection_focus, cx);
        }
    }

    pub(super) fn run(&mut self, intent: Intent, cx: &mut Context<Self>) {
        if self.is_settings_login()
            || self.disconnecting()
            || (intent == Intent::Guest && !self.state.can_leave_login())
        {
            return;
        }
        if let Intent::Disconnect(provider) = intent {
            self.cancel_account_favorite_changes(provider);
        }
        let request = self.state.begin(intent);
        self.execute_request(request, intent == Intent::Guest, cx);
    }

    pub(super) fn run_account(
        &mut self,
        provider: ProviderId,
        profile_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.is_settings_login() || self.disconnecting() {
            return;
        }
        let request = self.state.begin_account(provider, profile_id);
        self.execute_request(request, false, cx);
    }

    fn execute_request(&mut self, request: Request, open_guest: bool, cx: &mut Context<Self>) {
        self.stop_workspace_scrolling();
        self.login = None;
        self.connection_focus_pending = false;
        self.connection_motion.clear_flight();
        self.hovered = None;
        if !self.workspace.on_settings() {
            self.reset_scroll();
        }
        self.set_connection_open(open_guest);
        cx.spawn(async move |view, cx| {
            let work = request.clone();
            let (preview_tx, preview_rx) = async_channel::bounded(1);
            let task = cx.background_executor().spawn(async move {
                match Store::open_default() {
                    Ok(store) => {
                        if let Some(preview) = work.home_preview(&store) {
                            let _ = preview_tx.try_send(preview);
                        }
                        drop(preview_tx);
                        work.run(&store)
                    }
                    Err(error) => work.store_failure(error),
                }
            });
            if let Ok(preview) = preview_rx.recv().await {
                let _ = view.update(cx, |view, cx| {
                    if view.state.set_home_preview(&request, preview) {
                        view.sync_home_sources(cx);
                        cx.notify();
                    }
                });
            }
            let screen = task.await;
            let _ = view.update(cx, |view, cx| view.finish(&request, screen, cx));
        })
        .detach();
        cx.notify();
    }

    pub(super) fn submit_connection(
        &mut self,
        input: LoginInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(login) = &self.login else {
            return;
        };
        if matches!(login.phase, Phase::Working(_) | Phase::ChooseYouTube { .. }) {
            return;
        }
        let provider = login.provider;
        let intent = if self.is_settings_login() {
            Intent::Connect(provider)
        } else {
            Intent::SignIn(provider)
        };
        let request = self.state.begin(intent);
        self.perform_login(request, provider, input, window, cx);
    }

    fn choose_youtube(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = &mut self.login else {
            return;
        };
        if !matches!(login.phase, Phase::ChooseYouTube { .. }) {
            return;
        }
        if let Phase::ChooseYouTube { request, choices } =
            std::mem::replace(&mut login.phase, Phase::Setup)
        {
            self.perform_login(
                request,
                ProviderId::YouTubeMusic,
                LoginInput::ChooseYouTube { choices, id },
                window,
                cx,
            );
        }
    }

    fn perform_login(
        &mut self,
        request: Request,
        provider: ProviderId,
        input: LoginInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(login) = &mut self.login else {
            return;
        };
        login.phase = Phase::Working(None);
        // The activated submit button or profile choice leaves the tree.
        window.focus(&self.connection_focus, cx);
        cx.notify();
        let (send, receive) = async_channel::unbounded();
        let work = request.clone();
        cx.background_executor()
            .spawn(async move {
                let screen = match Store::open_default() {
                    Ok(store) => {
                        if let Some(screen) = work.login_preparation_failure(&store) {
                            let _ = send.try_send(Outcome::Done(screen));
                            return;
                        }
                        let input = match input {
                            LoginInput::YouTubeMusic { cookies } => {
                                match providers::youtube_profiles(cookies, work.cancel_flag()) {
                                    Ok(choices) if choices.profiles().len() == 1 => {
                                        let id = choices.profiles()[0].id.clone();
                                        LoginInput::ChooseYouTube { choices, id }
                                    }
                                    Ok(choices) => {
                                        let _ = send.try_send(Outcome::Profiles(choices));
                                        return;
                                    }
                                    Err(error) => {
                                        let _ = send.try_send(Outcome::Done(
                                            work.finish_login(&store, Err(error)),
                                        ));
                                        return;
                                    }
                                }
                            }
                            input => input,
                        };
                        let result =
                            providers::sign_in(provider, input, work.cancel_flag(), |prompt| {
                                let _ = send.try_send(Outcome::Prompt(prompt));
                            });
                        work.finish_login(&store, result)
                    }
                    Err(error) => work.store_failure(error),
                };
                let _ = send.try_send(Outcome::Done(screen));
            })
            .detach();
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(outcome) = receive.recv().await {
                if view
                    .update_in(cx, |view, window, cx| {
                        if !view.state.accepts(&request) {
                            return;
                        }
                        let Some(login) = &mut view.login else {
                            return;
                        };
                        match outcome {
                            Outcome::Prompt(prompt) => {
                                login.phase = Phase::Working(Some(prompt));
                                cx.notify();
                            }
                            Outcome::Profiles(choices) => {
                                login.phase = Phase::ChooseYouTube {
                                    request: request.clone(),
                                    choices,
                                };
                                cx.notify();
                            }
                            Outcome::Done(screen) => {
                                let restore_field = view.uses_fluid_glass()
                                    && youtube_validation_error(&screen).is_some()
                                    && view.connection_focus.is_focused(window);
                                view.finish(&request, screen, cx);
                                if restore_field {
                                    view.focus_connection(window, cx);
                                }
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn finish(&mut self, request: &Request, screen: Screen, cx: &mut Context<Self>) {
        let accounts_changed = matches!(
            (self.state.screen(), &screen),
            (
                Screen::Pending(
                    Intent::SignIn(_) | Intent::Connect(_) | Intent::Disconnect(_) | Intent::SwitchAccount
                ),
                Screen::Home(home)
            ) if home.switch_issue.is_none()
        ) || request.reconnected_provider().is_some();
        let disconnected = match (self.state.screen(), &screen) {
            (Screen::Pending(Intent::Disconnect(provider)), Screen::Home(_)) => Some(*provider),
            _ => None,
        };
        let modal_provider = if self.is_settings_login() {
            self.login.as_ref().map(Connection::provider)
        } else {
            None
        };
        let was_showing_cached_home = self.state.keeps_cached_preview(&screen);
        if !self.state.finish(request, screen) {
            return;
        }
        if let Some(provider) = disconnected {
            self.remove_account_favorites(provider, cx);
            self.forget_account_playlists(provider, None, cx);
        }
        if let Some(provider) = request.reconnected_provider() {
            self.invalidate_home_source(provider);
            self.forget_account_playlists(provider, None, cx);
        }
        if accounts_changed {
            self.clear_music_prefetch();
            self.forget_artist_discography();
            self.forget_track_release();
            self.clear_inspector_subject();
            self.cancel_related_music();
            self.lyrics_accounts_changed(cx);
            self.catalog_search_accounts_changed(cx);
            telemetry::catalog_invalidated(CatalogInvalidation::AccountChanged);
        }
        self.sync_like_accounts(cx);
        self.sync_home_sources(cx);
        match self.state.screen() {
            Screen::Failed(Failure {
                intent: Intent::SignIn(provider) | Intent::Connect(provider),
                ..
            })
            | Screen::Cancelled(Intent::SignIn(provider) | Intent::Connect(provider)) => {
                let provider = *provider;
                let login = self
                    .login
                    .get_or_insert_with(|| Connection::new(provider, cx));
                login.phase = Phase::Finished;
                self.set_connection_open(modal_provider.is_none());
            }
            _ => {
                self.login = None;
                self.settings_focus_pending = modal_provider;
                self.set_connection_open(self.guest_status());
            }
        }
        if !was_showing_cached_home && !self.workspace.on_settings() && !request.is_startup() {
            self.reset_scroll();
        }
        cx.notify();
    }

    pub(super) fn guest_status(&self) -> bool {
        matches!(
            self.state.screen(),
            Screen::Pending(Intent::Guest)
                | Screen::Cancelled(Intent::Guest)
                | Screen::Failed(Failure {
                    intent: Intent::Guest,
                    ..
                })
        )
    }

    pub(super) fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disconnecting() || matches!(self.state.screen(), Screen::Pending(Intent::Startup)) {
            return;
        }
        if self.is_settings_login() {
            self.close_settings_connection(window, cx);
        } else if let Some(login) = &self.login {
            if matches!(login.phase, Phase::Working(_) | Phase::ChooseYouTube { .. }) {
                self.cancel_connection(window, cx);
            } else {
                self.close_connection(window, cx);
            }
        } else {
            match self.state.screen() {
                Screen::Pending(_) => self.cancel(window, cx),
                Screen::Cancelled(_) | Screen::Failed(_) => self.show_welcome(window, cx),
                _ => {}
            }
        }
    }

    fn close_settings_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_settings_login()
            && self.state.dismiss_connection()
            && let Some(entry) = self.login.take()
        {
            self.set_connection_open(false);
            window.focus(&self.provider_focus[provider_index(entry.provider)], cx);
            cx.notify();
        }
    }

    fn close_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.can_leave_login() {
            return;
        }
        let provider = self.login.as_ref().map(Connection::provider);
        self.show_welcome(window, cx);
        if let Some(provider) = provider {
            window.focus(&self.provider_focus[provider_index(provider)], cx);
        }
    }

    fn show_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.reset();
        self.login = None;
        self.reset_scroll();
        self.set_connection_open(false);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn cancel_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.can_leave_login() {
            return;
        }
        self.state.cancel();
        if let Some(login) = &mut self.login {
            login.phase = Phase::Finished;
        }
        window.focus(&self.connection_focus, cx);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.cancel();
        self.reset_scroll();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn retry_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modal = self.is_settings_login();
        let Some(login) = &mut self.login else {
            return;
        };
        match login.provider {
            ProviderId::Spotify => self.submit_connection(LoginInput::Spotify, window, cx),
            ProviderId::YandexMusic => self.submit_connection(LoginInput::YandexDevice, window, cx),
            ProviderId::SoundCloud => {
                self.submit_connection(soundcloud_input(window), window, cx);
            }
            _ => {
                if !modal {
                    self.state.reset();
                }
                login.phase = Phase::Setup;
                self.focus_connection(window, cx);
                cx.notify();
            }
        }
    }

    fn retry(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        match intent {
            Intent::SignIn(provider) => self.select_provider(provider, window, cx),
            _ => self.run(intent, cx),
        }
    }

    pub(super) fn screen(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .min_h_full()
            .flex()
            .items_center()
            .justify_center()
            .px(px(25.))
            .py(px(25.))
            .child(self.route_panel(cx))
    }

    fn route_panel(&self, cx: &Context<Self>) -> Div {
        match self.state.screen() {
            Screen::Welcome | Screen::Home(_) => div(),
            Screen::Pending(intent) => self
                .route_surface(cx)
                .child(heading(pending_title(*intent), 25.))
                .child(message("Получаем данные…", cx)),
            Screen::Cancelled(intent) => self.outcome_panel("Действие отменено", None, *intent, cx),
            Screen::Failed(fail) => self.outcome_panel(
                "Не удалось продолжить",
                Some(fail.issue.to_string()),
                fail.intent,
                cx,
            ),
        }
    }

    pub(super) fn guest_connection_panel(
        &self,
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        self.route_panel(cx)
            .id("login-panel")
            .accessibility_id("login-panel")
            .role(Role::Group)
            .aria_label("Гостевой вход")
            .relative()
            .size_full()
            .when(compact, |panel| {
                panel.pt(px(56.)).child(
                    div()
                        .absolute()
                        .top(px(13.))
                        .left(px(19.))
                        .child(self.connection_return(cx)),
                )
            })
    }
    fn outcome_panel(
        &self,
        title: &'static str,
        detail: Option<String>,
        intent: Intent,
        cx: &Context<Self>,
    ) -> Div {
        self.route_surface(cx)
            .child(heading(title, 26.))
            .children(detail.map(|detail| message(detail, cx)))
            .child(
                actions_row().child(
                    Button::new("retry", "Повторить")
                        .variant(Variant::Light)
                        .when_some(self.glass_controls(cx), |button, material| {
                            ControlVariant::DialogAction.dialog_button(button, material)
                        })
                        .tab_order(1)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.retry(intent, window, cx)),
                        ),
                ),
            )
    }

    fn connection_return(&self, cx: &Context<Self>) -> impl IntoElement {
        let glass = self.glass_controls(cx);
        button("connection-return", "Вернуться к списку", 18)
            .size(px(32.))
            .flex_none()
            .justify_center()
            .rounded(px(9.))
            .when_some(glass, |control, glass| glass.paint(control, Tone::Quiet))
            .when(glass.is_none(), |control| {
                control.hover(|style| style.bg(icon_button_hover()))
            })
            .child(
                Icon::new(IconName::ArrowLeft)
                    .pixel_size(px(19.))
                    .color(theme(cx).text().hsla()),
            )
            .on_activate(cx.listener(|this, _, window, cx| this.close_connection(window, cx)))
    }

    fn route_surface(&self, cx: &Context<Self>) -> Div {
        div()
            .w_full()
            .max_w(px(500.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .rounded(px(10.))
            .bg(theme(cx).surface().hsla())
            .when_some(self.glass_controls(cx), |surface, glass| {
                glass.group(surface.border_1())
            })
    }
}

fn pending_title(intent: Intent) -> &'static str {
    match intent {
        Intent::Startup => "Открываем Mediadeka",
        Intent::Guest => "Открываем гостевую подборку",
        Intent::SignIn(provider) | Intent::Connect(provider) => provider.display_name(),
        Intent::Disconnect(_) => "Отключаем площадку",
        Intent::SwitchAccount => "Открываем площадку",
        Intent::Refresh => "Обновляем музыку",
    }
}

fn provider_heading(provider: ProviderId) -> Div {
    let path = ProviderId::ALL
        .into_iter()
        .zip(BRAND_MARKS)
        .find_map(|(id, path)| (id == provider).then_some(path))
        .expect("provider mark");
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(mark(path, px(25.)))
        .child(
            div()
                .id("login-provider-name")
                .accessibility_id("login-provider-name")
                .role(Role::Heading)
                .aria_label(provider.display_name())
                .text_size(px(25.))
                .line_height(px(32.))
                .font_weight(FontWeight(670.))
                .child(provider.display_name()),
        )
}

fn message(text: impl Into<gpui::SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(15.))
        .line_height(px(22.))
        .text_color(theme(cx).dimmed())
        .child(text.into())
}

fn status_message(id: &'static str, text: String, cx: &App) -> impl IntoElement {
    message(text.clone(), cx)
        .id(id)
        .accessibility_id(id)
        .role(Role::Label)
        .aria_value(text)
}

fn actions_row() -> Div {
    div().flex().flex_wrap().items_center().gap(px(8.))
}

fn open_link(
    id: &'static str,
    label: &'static str,
    url: String,
    order: isize,
    glass: Option<Material>,
    variant: ControlVariant,
) -> impl IntoElement {
    let ink = glass.map_or_else(primary, |material| {
        variant.resolve(material, ControlState::default()).fg
    });
    Button::new(id, label)
        .variant(Variant::Light)
        .when_some(glass, |button, material| {
            if variant == ControlVariant::DialogAction {
                variant.dialog_button(button, material)
            } else {
                variant.text_button(button, material)
            }
        })
        .tab_order(order)
        .right_section(
            Icon::new(IconName::ExternalLink)
                .pixel_size(px(15.))
                .color(ink),
        )
        .on_click(move |_, _, cx| open_web(&url, cx))
}

fn open_web(url: &str, cx: &App) {
    if url::Url::parse(url)
        .is_ok_and(|url| matches!(url.scheme(), "https" | "http") && url.has_host())
    {
        cx.open_url(url);
    }
}
