mod fluid;
pub(super) mod reveal;

use gpui::{
    AccessibleAction, ClickEvent, Context, Div, FocusHandle, FontWeight, Hsla, Role, Stateful,
    Task, Window, WindowControlArea, div, linear_color_stop, linear_gradient, point, prelude::*,
    px, relative, rgb,
};
use guise::{Icon, IconName};

use super::{
    MainView,
    components::{Activate, artwork, button, glass_control::Tone},
    design_system::{
        controls::{paint_page_tab, tab_selection_marker},
        materials::Material,
        motion::{MOTION, Slide},
    },
    fluid_glass::{
        tokens::{size, space, text},
        transition_motion_configured,
    },
    music::detail::hero_motion::{HeroMotion, IMMERSIVE_ARTWORK_SPAN, PLAYER_ARTWORK_SPAN},
    theme::{self, playlist},
    window_controls,
};

pub(super) const WIDE_TEXT: f32 = 1100.;
const BASE: u32 = 0x110f0e;
const COVER_TAB_ORDER: isize = 9;
const LYRICS_TAB_ORDER: isize = 10;
const LISTENING_TAB_WIDTH: f32 = 76.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ListeningMode {
    Cover,
    Lyrics,
}

impl ListeningMode {
    fn tab_slot(self) -> f32 {
        match self {
            Self::Cover => 0.,
            Self::Lyrics => 1.,
        }
    }
}

pub(super) struct Listening {
    focus: FocusHandle,
    cover_tab: FocusHandle,
    lyrics_tab: FocusHandle,
    session: Option<Session>,
    tab_slide: Option<Slide>,
    artwork_motion: Option<HeroMotion>,
    reveal_frames: Option<Task<()>>,
    #[cfg(feature = "capture")]
    artwork_sample: Option<std::time::Duration>,
    #[cfg(feature = "capture")]
    native_fullscreen_for_capture: bool,
}

struct Session {
    mode: ListeningMode,
    previous_focus: Option<FocusHandle>,
    fullscreen_entered: bool,
    lyrics_requested: bool,
}

impl Listening {
    pub(super) fn new(cx: &mut Context<MainView>) -> Self {
        Self {
            focus: cx.focus_handle(),
            cover_tab: cx.focus_handle().tab_index(COVER_TAB_ORDER).tab_stop(true),
            lyrics_tab: cx.focus_handle().tab_index(LYRICS_TAB_ORDER).tab_stop(true),
            session: None,
            tab_slide: None,
            artwork_motion: None,
            reveal_frames: None,
            #[cfg(feature = "capture")]
            artwork_sample: None,
            #[cfg(feature = "capture")]
            native_fullscreen_for_capture: false,
        }
    }

    fn tab_focus(&self, mode: ListeningMode) -> &FocusHandle {
        match mode {
            ListeningMode::Cover => &self.cover_tab,
            ListeningMode::Lyrics => &self.lyrics_tab,
        }
    }

    pub(super) fn mode(&self) -> Option<ListeningMode> {
        self.session.as_ref().map(|session| session.mode)
    }

    pub(super) fn lyrics_requested(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.lyrics_requested)
    }
}

impl MainView {
    pub(super) fn toggle_listening(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.listening.mode().is_some() {
            self.set_listening_mode(None, window, cx);
        } else if self.state.home_view().is_some()
            && !self.is_settings_login()
            && !self.soloist.is_open()
            && self.like_menu_focus().is_none()
        {
            self.set_listening_mode(Some(ListeningMode::Cover), window, cx);
        }
    }

    pub(super) fn set_listening_mode(
        &mut self,
        mode: Option<ListeningMode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_listening_mode(mode, false, window, cx);
    }

    fn listening_tabs_animate(&self, cx: &Context<Self>) -> bool {
        self.uses_fluid_glass()
            && transition_motion_configured()
            && !self.reduced_motion
            && !cx.reduce_motion()
            && !self.fluid_high_contrast(cx)
    }

    fn change_listening_mode(
        &mut self,
        mode: Option<ListeningMode>,
        pointer: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.listening.mode();
        if previous == mode {
            return;
        }
        self.listening.tab_slide = previous
            .zip(mode)
            .filter(|_| pointer && self.listening_tabs_animate(cx))
            .map(|(previous, next)| {
                let now = self.presentation_time();
                let from = self
                    .listening
                    .tab_slide
                    .map_or_else(|| previous.tab_slot(), |slide| slide.position(now));
                Slide::new(from, next.tab_slot(), now, MOTION.fast())
            });
        let entering = previous.is_none() && mode.is_some();
        let source = if previous.is_some() {
            IMMERSIVE_ARTWORK_SPAN
        } else {
            PLAYER_ARTWORK_SPAN
        };
        let target = if mode.is_some() {
            IMMERSIVE_ARTWORK_SPAN
        } else {
            PLAYER_ARTWORK_SPAN
        };
        #[cfg(feature = "capture")]
        let phase = self.listening.artwork_sample;
        #[cfg(not(feature = "capture"))]
        let phase: Option<std::time::Duration> = None;
        let artwork_motion = (self.uses_fluid_glass()
            && transition_motion_configured()
            && !self.reduced_motion
            && !self.fluid_high_contrast(cx)
            && (self.capture_time.is_none() || phase.is_some())
            && self.player.read(cx).current().is_some())
        .then(|| window.rendered_span(source))
        .flatten()
        .map(|origin| HeroMotion::with_duration(origin, target, MOTION.immersive()));
        #[cfg(feature = "capture")]
        let artwork_motion = artwork_motion.map(|mut motion| {
            if let Some(phase) = phase {
                motion.sample(phase);
            }
            motion
        });
        self.listening.artwork_motion = artwork_motion;
        if entering || mode.is_none() {
            self.listening.reveal_frames.take();
        }
        if mode.is_some()
            && entering
            && artwork_motion.is_some_and(|motion| motion.active() && !motion.sampled())
        {
            // One progress source for live and capture: both the cover and
            // the staged groups read this HeroMotion. The task only asks for
            // frames while the live motion is running.
            let timer = cx.background_executor().clone();
            self.listening.reveal_frames = Some(cx.spawn(async move |view, cx| {
                loop {
                    timer.timer(std::time::Duration::from_millis(16)).await;
                    let active = view.update(cx, |view, cx| {
                        let active = view
                            .listening
                            .artwork_motion
                            .is_some_and(|motion| motion.active() && !motion.sampled());
                        if active {
                            cx.notify();
                        }
                        active
                    });
                    if !active.unwrap_or(false) {
                        break;
                    }
                }
            }));
        }
        let lyrics = mode == Some(ListeningMode::Lyrics);
        let allow_fullscreen = self.capture_time.is_none();
        #[cfg(feature = "capture")]
        let allow_fullscreen = allow_fullscreen || self.listening.native_fullscreen_for_capture;
        let focus = match (mode, self.listening.session.as_mut()) {
            (Some(mode), Some(session)) => {
                session.mode = mode;
                session.lyrics_requested |= lyrics;
                Some(self.listening.tab_focus(mode).clone())
            }
            (Some(mode), None) => {
                let fullscreen_entered = allow_fullscreen && !window.is_fullscreen();
                self.listening.session = Some(Session {
                    mode,
                    previous_focus: window.focused(cx),
                    fullscreen_entered,
                    lyrics_requested: lyrics,
                });
                if fullscreen_entered {
                    window.toggle_fullscreen();
                }
                Some(self.listening.focus.clone())
            }
            (None, _) => self.listening.session.take().map(|session| {
                if session.fullscreen_entered && window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                session.previous_focus.unwrap_or_else(|| self.focus.clone())
            }),
        };
        self.lyrics.update(cx, |view, cx| {
            if lyrics {
                view.set_inspection(None, cx);
            }
            view.set_immersive(lyrics, cx);
        });
        self.player.update(cx, |bar, cx| {
            bar.set_listening_artwork_motion(artwork_motion.filter(|_| mode.is_none()), cx);
            if entering {
                bar.set_listening_reveal(artwork_motion, cx);
            } else if mode.is_none() {
                bar.set_listening_reveal(None, cx);
            }
            bar.set_listening_presentation(mode.is_some(), cx);
        });
        self.sync_panel_buttons(cx);
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn allow_native_listening_fullscreen_for_capture(&mut self) {
        self.listening.native_fullscreen_for_capture = true;
    }

    #[cfg(feature = "capture")]
    pub fn sample_listening_artwork_for_capture(
        &mut self,
        phase: Option<std::time::Duration>,
        cx: &mut Context<Self>,
    ) {
        self.listening.artwork_sample = phase;
        if let Some(phase) = phase {
            if let Some(motion) = self.listening.artwork_motion.as_mut() {
                motion.sample(phase);
            }
        } else {
            self.listening.artwork_motion = None;
        }
        let mode = self.listening.mode();
        if mode.is_none() {
            let motion = self.listening.artwork_motion;
            self.player.update(cx, |bar, cx| {
                bar.set_listening_artwork_motion(motion, cx);
                bar.set_listening_reveal(None, cx);
            });
        } else {
            let motion = self.listening.artwork_motion;
            self.player.update(cx, |bar, cx| {
                bar.set_listening_reveal(motion, cx);
            });
        }
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn listening_artwork_bounds_for_capture(
        &self,
        window: &Window,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        window.rendered_span(if self.listening.mode().is_some() {
            IMMERSIVE_ARTWORK_SPAN
        } else {
            PLAYER_ARTWORK_SPAN
        })
    }

    /// Opacity and paint-only offset of the three staged groups at the sampled
    /// phase. Native acceptance reads the same values the frame was drawn with.
    #[cfg(feature = "capture")]
    pub fn listening_reveal_for_capture(&self) -> ([f32; 2], [f32; 2], [f32; 2]) {
        let motion = self.listening.artwork_motion;
        let pair = |(opacity, offset)| [opacity, offset];
        (
            pair(reveal::metadata(motion)),
            pair(reveal::progress(motion)),
            pair(reveal::transport(motion)),
        )
    }

    pub(super) fn listening_view(
        &self,
        mode: ListeningMode,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if self.uses_fluid_glass() {
            return self.fluid_listening_view(mode, window, cx);
        }
        let player = self.player.read(cx);
        let track = player.current().map(|entry| &entry.track);
        let title = track.map_or("Сейчас ничего не играет", |track| {
            track.title.as_str()
        });
        let artist = track
            .map(|track| track.artist.as_str())
            .filter(|name| !name.is_empty());
        let cover = player.current_artwork();
        let compact = window.viewport_size().width < px(WIDE_TEXT);
        let lyrics = mode == ListeningMode::Lyrics;
        let heading =
            |scale, line, lines| track_heading(title, artist, scale, line, lines, theme::SECONDARY);
        self.listening_container(mode)
            .text_color(rgb(theme::PRIMARY))
            .bg(rgb(BASE))
            .child(backdrop(cover, lyrics))
            .child(self.listening_header(mode, compact, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .relative()
                    .flex()
                    .gap(px(if compact { 18. } else { 56. }))
                    .px(px(if compact { 22. } else { 68. }))
                    .when(!lyrics, |stage| {
                        stage
                            .items_end()
                            .pb(px(if compact { 28. } else { 86. }))
                            .child(div().w_full().max_w(px(1760.)).child(heading(
                                if compact { 38. } else { 72. },
                                if compact { 43. } else { 77. },
                                if compact { 3 } else { 2 },
                            )))
                    })
                    .when(lyrics && !compact, |stage| {
                        stage.pt(px(26.)).pb(px(18.)).child(
                            div()
                                .w(relative(0.34))
                                .flex_none()
                                .pt(px(72.))
                                .child(heading(48., 54., 2)),
                        )
                    })
                    .when(lyrics, |stage| {
                        stage.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .child(self.lyrics.clone()),
                        )
                    }),
            )
            .child(div().flex_none().child(self.player.clone()))
    }

    fn listening_container(&self, mode: ListeningMode) -> Stateful<Div> {
        let lyrics = mode == ListeningMode::Lyrics;
        div()
            .id(if lyrics {
                "lyrics-fullscreen"
            } else {
                "listening-fullscreen"
            })
            .accessibility_id(if lyrics {
                "lyrics-fullscreen"
            } else {
                "listening-fullscreen"
            })
            .role(Role::Group)
            .aria_label(if lyrics {
                "Полноэкранный текст песни"
            } else {
                "Полноэкранное прослушивание"
            })
            .track_focus(&self.listening.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
    }

    fn listening_header(
        &self,
        mode: ListeningMode,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let glass = self.glass_controls(cx);
        div()
            .relative()
            .w_full()
            .h(px(if compact { 50. } else { 72. }))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(if compact { 10. } else { 24. }))
            .px(px(if compact { 18. } else { 31. }))
            .child(
                header_wing()
                    .child(
                        button("listening-back", "Вернуться к прежнему экрану", 8)
                            .h(px(36.))
                            .px(px(8.))
                            .gap(px(8.))
                            .rounded(px(6.))
                            .map(|control| match glass {
                                Some(material) => material.paint(control, Tone::Quiet),
                                None => control
                                    .text_color(rgb(theme::SECONDARY))
                                    .hover(|state| state.bg(rgb(playlist::HOVER))),
                            })
                            .child(Icon::new(IconName::ChevronLeft).pixel_size(px(18.)))
                            .child("Назад")
                            .on_activate(cx.listener(|view, _, window, cx| {
                                view.set_listening_mode(None, window, cx);
                            })),
                    )
                    .child(header_drag_area()),
            )
            .child(
                div()
                    .id("listening-tabs")
                    .accessibility_id("listening-tabs")
                    .role(Role::TabList)
                    .aria_label("Вид полноэкранного прослушивания")
                    .flex()
                    .flex_none()
                    .when(glass.is_some(), |tabs| tabs.gap(px(space::XS)))
                    .when(glass.is_none(), |tabs| {
                        tabs.gap(px(3.))
                            .p(px(3.))
                            .rounded(px(8.))
                            .bg(Hsla::from(rgb(BASE)).alpha(0.72))
                            .border_1()
                            .border_color(Hsla::from(rgb(theme::DIVIDER)).alpha(0.8))
                    })
                    .child(self.listening_tab(
                        mode,
                        ListeningMode::Cover,
                        "listening-cover",
                        "Обложка",
                        COVER_TAB_ORDER,
                        cx,
                    ))
                    .child(self.listening_tab(
                        mode,
                        ListeningMode::Lyrics,
                        "listening-text",
                        "Текст",
                        LYRICS_TAB_ORDER,
                        cx,
                    ))
                    .when_some(glass, |tabs, glass| {
                        tabs.relative()
                            .child(self.listening_tab_marker(mode, glass, window, cx))
                    }),
            )
            .child(
                header_wing()
                    .child(header_drag_area())
                    .when(!window.is_fullscreen(), |wing| {
                        wing.child(window_controls(window, cx))
                    }),
            )
    }

    fn listening_tab_marker(
        &self,
        mode: ListeningMode,
        glass: Material,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let now = self.presentation_time();
        let slide = self
            .listening
            .tab_slide
            .filter(|slide| slide.active(now) && self.listening_tabs_animate(cx));
        if slide.is_some()
            && self.capture_time.is_none()
            && window.is_visible()
            && window.is_window_active()
        {
            window.request_animation_frame();
        }
        let slot = slide.map_or_else(|| mode.tab_slot(), |slide| slide.position(now));
        div()
            .absolute()
            .bottom(px(2.))
            .left_0()
            .w(px(LISTENING_TAB_WIDTH))
            .flex()
            .justify_center()
            .layer_translate(point(px(slot * (LISTENING_TAB_WIDTH + space::XS)), px(0.)))
            .child(tab_selection_marker(glass))
    }

    fn listening_tab(
        &self,
        mode: ListeningMode,
        tab: ListeningMode,
        id: &'static str,
        label: &'static str,
        order: isize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let selected = mode == tab;
        let control = button(id, label, order)
            .track_focus(self.listening.tab_focus(tab))
            .role(Role::Tab)
            .aria_selected(selected);
        let control = if let Some(material) = self.glass_controls(cx) {
            paint_page_tab(control, material, selected)
                .w(px(LISTENING_TAB_WIDTH))
                .h(px(size::SM))
                .justify_center()
                .text_size(px(text::BODY))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
        } else {
            control
                .h(px(30.))
                .px(px(13.))
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(rgb(theme::SECONDARY))
                .when(selected, |button| {
                    button
                        .bg(rgb(theme::ELEVATED))
                        .text_color(rgb(theme::PRIMARY))
                })
                .hover(|state| state.bg(rgb(theme::ELEVATED)))
        };
        let activate = cx.listener(move |view, _: &(), window, cx| {
            view.set_listening_mode(Some(tab), window, cx);
        });
        control
            .child(label)
            .on_click(cx.listener(move |view, event: &ClickEvent, window, cx| {
                let pointer = !matches!(event, ClickEvent::Keyboard(_));
                view.change_listening_mode(Some(tab), pointer, window, cx);
            }))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                activate(&(), window, cx)
            })
    }
}

fn header_wing() -> Div {
    div().h_full().flex_1().min_w_0().flex().items_center()
}

fn header_drag_area() -> Div {
    div()
        .h_full()
        .flex_1()
        .window_control_area(WindowControlArea::Drag)
}

fn track_heading(
    title: &str,
    artist: Option<&str>,
    size: f32,
    line: f32,
    lines: usize,
    secondary: u32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(12.))
        .child(
            div()
                .id("listening-track")
                .accessibility_id("listening-track")
                .role(Role::Heading)
                .aria_label(title.to_owned())
                .font_weight(FontWeight(680.))
                .text_size(px(size))
                .line_height(px(line))
                .when(lines == 1, |label| label.truncate())
                .when(lines > 1, |label| label.line_clamp(lines).text_ellipsis())
                .tooltip(guise::tooltip(title.to_owned()))
                .child(title.to_owned()),
        )
        .children(artist.map(|name| {
            div()
                .text_size(px(if size > 42. { 21. } else { 15. }))
                .text_color(rgb(secondary))
                .truncate()
                .child(name.to_owned())
        }))
}

fn backdrop(cover: Option<&str>, lyrics: bool) -> Div {
    let base = Hsla::from(rgb(BASE));
    div()
        .absolute()
        .inset_0()
        .when_some(cover, |frame, cover| {
            frame
                .child(artwork::remote(cover, false, true))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(base.alpha(if lyrics { 0.82 } else { 0.30 })),
                )
                .child(div().absolute().inset_0().bg(linear_gradient(
                    180.,
                    linear_color_stop(base.alpha(0.), 0.15),
                    linear_color_stop(base, 1.),
                )))
                .child(div().absolute().inset_0().bg(linear_gradient(
                    90.,
                    linear_color_stop(base.alpha(if lyrics { 0.8 } else { 0.9 }), 0.),
                    linear_color_stop(base.alpha(0.), 0.78),
                )))
        })
        .when(cover.is_none(), |frame| {
            frame.bg(linear_gradient(
                148.,
                linear_color_stop(Hsla::from(rgb(theme::ELEVATED)).alpha(0.66), 0.),
                linear_color_stop(base, 0.75),
            ))
        })
}
