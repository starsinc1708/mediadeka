use gpui::{
    ClipboardItem, Context, Div, ElementId, FocusHandle, KeyDownEvent, Role, SharedString, Window,
    div, prelude::*, px, rgb,
};
use guise::IconName;

use crate::{
    appearance::{TrackColumn, TrackDensity},
    music::likes::Subject,
    playback::{QueueEntry, can_resolve},
    playlist::{Media, PlaylistKey},
};

use crate::ui::{
    ShiftTab, Tab,
    components::overlay::{self, MenuLayout, TriggerBounds},
    design_system::{
        controls::{ControlState, ControlVariant},
        icons::{Glyph, animated_glyph},
    },
    theme::{ACCENT, DIVIDER, ELEVATED, PLAYLIST_MENU_HOVER, PLAYLIST_MENU_SELECTED, playlist},
};

use super::{
    MainView,
    actions::{Edit, ImportPaths, QueueAction},
    navigation::{self, Destination, track_links},
};
use crate::ui::components::{Activate, button};

const WIDTH: f32 = 235.;
const LINKS_WIDTH: f32 = 278.;
const ITEM_HEIGHT: f32 = 33.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum MenuKind {
    Playlist,
    Track(usize),
    LibraryTrack(usize),
    ObjectTrack(usize),
    ObjectLinks { can_search: bool },
    ObjectHero { playable: bool },
    NowPlaying,
    Import,
    Columns { album: bool, date: bool },
}

pub(super) struct Menu {
    kind: MenuKind,
    bounds: TriggerBounds,
    restore: Option<FocusHandle>,
    selected: usize,
    radio: Option<QueueEntry>,
    recording: Option<QueueEntry>,
}

#[derive(Clone)]
enum Command {
    Queue(QueueAction, Option<usize>),
    TrackRadio,
    Details(usize),
    Navigate(Destination),
    LibraryLikeDestinations,
    OpenSource(String),
    Density(TrackDensity),
    Album,
    Date,
    CopyLink(String),
    CopyMusicLink { platform: String, url: String },
    ToggleMusicPlatformSearch,
    ObjectAppend,
    ObjectRadio,
    PinObject(crate::ui::workspace::pins::PinRequest),
    Refresh,
    RemoveLocalTrack(usize),
    AddFiles(i64),
    Import(ImportPaths),
    Edit(Edit),
}

struct Item {
    title: SharedString,
    glyph: IconName,
    command: Command,
    separator: bool,
}

impl Item {
    fn new(title: impl Into<SharedString>, glyph: IconName, command: Command) -> Self {
        Self {
            title: title.into(),
            glyph,
            command,
            separator: false,
        }
    }

    fn separated(mut self, separate: bool) -> Self {
        self.separator = separate;
        self
    }
}

impl MainView {
    pub(in crate::ui) fn playlist_menu_trigger(
        &self,
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        glyph: IconName,
        order: isize,
        kind: MenuKind,
        cx: &Context<MainView>,
    ) -> Div {
        let id = id.into();
        // Keep the same measured bounds alive while this menu is open, so the
        // panel follows its trigger through scrolling and window resizes.
        let bounds = self
            .playlist
            .menu
            .as_ref()
            .filter(|menu| menu.kind == kind)
            .map(|menu| menu.bounds.clone())
            .unwrap_or_default();
        let anchor = bounds.clone();
        let expanded = self
            .playlist
            .menu
            .as_ref()
            .is_some_and(|menu| menu.kind == kind);
        let activate = cx.listener(move |view, _, window, cx| {
            cx.stop_propagation();
            if view
                .playlist
                .menu
                .as_ref()
                .is_some_and(|menu| menu.kind == kind)
            {
                view.dismiss_playlist_overlay(window, cx);
            } else {
                view.show_playlist_menu(kind, anchor.clone(), window, cx);
            }
        });
        let material = self.glass_controls(cx);
        let title = title.into();
        let trigger = if kind == MenuKind::Import {
            overlay::select_trigger(
                "playlist-import",
                title.clone(),
                title,
                order,
                expanded,
                material,
                cx,
            )
        } else {
            let size = if matches!(
                kind,
                MenuKind::Track(_) | MenuKind::ObjectTrack(_) | MenuKind::LibraryTrack(_)
            ) {
                playlist::metrics::CONTROL_SMALL
            } else {
                playlist::metrics::CONTROL
            };
            let trigger = button(id.clone(), title.clone(), order)
                .size(px(size))
                .flex_none()
                .justify_center();
            if let Some(material) = material {
                let trigger = if matches!(glyph, IconName::Ellipsis | IconName::EllipsisVertical) {
                    trigger.child(animated_glyph(id, Glyph::More, expanded, 18., None, cx))
                } else {
                    trigger.child(overlay::menu_icon(glyph))
                };
                ControlVariant::GhostIcon
                    .paint(trigger, material, ControlState::default())
                    .when(!expanded, |trigger| trigger.tooltip(guise::tooltip(title)))
            } else {
                trigger
                    .rounded(px(playlist::RADIUS))
                    .text_color(rgb(playlist::INK_SECONDARY))
                    .hover(|style| style.bg(rgb(playlist::HOVER)))
                    .child(overlay::menu_icon(glyph))
                    .when(!expanded, |trigger| trigger.tooltip(guise::tooltip(title)))
            }
        };
        let trigger = if kind == MenuKind::Import && self.playlist.local_busy {
            trigger
                .tab_stop(false)
                .cursor_default()
                .opacity(0.55)
                .aria_description("Импорт выполняется")
        } else {
            trigger.on_toggle(expanded, activate)
        };
        bounds.capture(trigger)
    }

    pub(in crate::ui) fn show_playlist_menu(
        &mut self,
        kind: MenuKind,
        bounds: TriggerBounds,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_settings_login() || self.disconnecting() {
            return;
        }
        let radio = match kind {
            MenuKind::Track(index) => self
                .playlist
                .open
                .as_ref()
                .and_then(|open| open.queue_entry(index)),
            MenuKind::ObjectTrack(index) => self.object_radio_entry(index),
            _ => None,
        }
        .filter(|entry| entry.is_playable() && self.can_start_track_radio(entry));
        let recording = match kind {
            MenuKind::NowPlaying => self.player.read(cx).current_entry().cloned(),
            MenuKind::LibraryTrack(index) => self.library_recording(index),
            _ => None,
        };
        if matches!(kind, MenuKind::NowPlaying | MenuKind::LibraryTrack(_)) && recording.is_none() {
            return;
        }
        self.dismiss_lyrics_picker(window, cx);
        let restore = window.focused(cx);
        let ui = &mut self.playlist;
        let selected = 0;
        ui.menu = Some(Menu {
            kind,
            bounds,
            restore,
            selected,
            radio,
            recording,
        });
        window.focus(&ui.menu_focus, cx);
        cx.notify();
    }

    pub(in crate::ui) fn dismiss_playlist_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ui = &mut self.playlist;
        if let Some(menu) = ui.menu.take() {
            if let Some(restore) = menu.restore {
                window.focus(&restore, cx);
            }
            cx.notify();
            true
        } else {
            self.dismiss_local_editor(window, cx)
        }
    }

    pub(in crate::ui) fn playlist_menu_open(&self) -> bool {
        self.playlist.menu.is_some()
    }

    pub(in crate::ui) fn now_playing_menu_open(&self) -> bool {
        self.playlist
            .menu
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::NowPlaying)
    }

    pub(in crate::ui) fn playlist_overlay(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let menu = self.playlist.menu.as_ref()?;
        let material = self.glass_controls(cx);
        let playlist_menu = !matches!(
            menu.kind,
            MenuKind::ObjectTrack(_) | MenuKind::ObjectLinks { .. } | MenuKind::ObjectHero { .. }
        );
        let width = if matches!(menu.kind, MenuKind::ObjectLinks { .. }) {
            LINKS_WIDTH
        } else {
            WIDTH
        };
        let focused = self.playlist.menu_focus.is_focused(window);
        let items = self.menu_items(menu);
        let viewport = window.viewport_size();
        let classic_height =
            material
                .is_none()
                .then_some(if playlist_menu && viewport.height <= px(500.) {
                    29.
                } else {
                    ITEM_HEIGHT
                });
        let layout = MenuLayout::new(window, width, classic_height);
        let (selected, hover, rule) = if playlist_menu {
            (playlist::SELECTED, playlist::HOVER, playlist::RULE)
        } else {
            (PLAYLIST_MENU_SELECTED, PLAYLIST_MENU_HOVER, DIVIDER)
        };
        let count = items.len();
        let rules = items.iter().filter(|item| item.separator).count();
        let cursor = menu.selected.min(count.saturating_sub(1));
        let panel = layout
            .frame(
                "playlist-menu",
                if menu.kind == MenuKind::Import {
                    "Импорт"
                } else {
                    "Действия"
                },
                material,
            )
            .track_focus(&self.playlist.menu_focus)
            .on_action(cx.listener(|view, _: &Tab, window, cx| {
                view.dismiss_playlist_overlay(window, cx);
            }))
            .on_action(cx.listener(|view, _: &ShiftTab, window, cx| {
                view.dismiss_playlist_overlay(window, cx);
            }))
            .on_key_down(
                cx.listener(|view, event, window, cx| view.playlist_menu_key(event, window, cx)),
            )
            .when(material.is_none(), |panel| {
                panel.border_color(rgb(rule)).bg(rgb(if playlist_menu {
                    playlist::POPOVER
                } else {
                    ELEVATED
                }))
            })
            .when(items.is_empty(), |panel| {
                panel.child(
                    div()
                        .h(px(layout.item_height))
                        .pl(px(8.))
                        .text_size(px(13.))
                        .child("Нет доступных действий"),
                )
            })
            .children(items.into_iter().enumerate().map(|(index, item)| {
                let id: ElementId = match (menu.kind, &item.command) {
                    (_, Command::TrackRadio) => "track-radio-action".into(),
                    (_, Command::Import(ImportPaths::Files)) => "playlist-import-files".into(),
                    (_, Command::Import(ImportPaths::Folder)) => "playlist-import-folder".into(),
                    (_, Command::Import(ImportPaths::M3u)) => "playlist-import-m3u".into(),
                    (_, Command::Density(density)) => {
                        format!("playlist-density-{}", density.as_str()).into()
                    }
                    (MenuKind::LibraryTrack(row), Command::Navigate(_)) => {
                        format!("library-open-track-{row}").into()
                    }
                    _ => ("playlist-menu-action", index).into(),
                };
                let pointed = index == cursor;
                let checked = match item.command {
                    Command::Density(density) => Some(self.track_density == density),
                    _ => None,
                };
                let row = button(id, item.title.clone(), 400 + index as isize)
                    .tab_stop(false)
                    .role(Role::MenuItem)
                    .when(pointed && focused, |row| row.aria_active_descendant())
                    .when_some(checked, |row, checked| row.aria_selected(checked));
                let row = if let Some(material) = material {
                    overlay::menu_item(row, layout, material, pointed && focused)
                } else {
                    row.w_full()
                        .h(px(layout.item_height))
                        .flex_none()
                        .px(px(7.))
                        .gap(px(8.))
                        .rounded(px(5.))
                        .text_size(px(13.))
                        .hover(move |style| style.bg(rgb(hover)))
                        .when(pointed, |row| {
                            row.bg(rgb(selected)).when(playlist_menu && focused, |row| {
                                row.border_color(rgb(playlist::SELECTION_EDGE))
                            })
                        })
                };
                let row = row.when(checked.is_none(), |row| {
                    row.child(
                        overlay::menu_icon(item.glyph).when(material.is_none(), |icon| {
                            icon.color(gpui::Hsla::from(rgb(if playlist_menu {
                                playlist::SIGNAL
                            } else {
                                ACCENT
                            })))
                        }),
                    )
                });
                let rule = if let Some(material) = material {
                    material
                        .kind(crate::ui::design_system::materials::Kind::SmokedGlass)
                        .style(crate::ui::design_system::materials::Interaction::Rest)
                        .edge
                        .unwrap_or_else(gpui::transparent_black)
                } else {
                    rgb(rule).into()
                };
                div()
                    .when(item.separator, |group| group.child(layout.rule(rule)))
                    .child(
                        overlay::menu_value(row, item.title, checked == Some(true)).on_activate(
                            cx.listener(move |view, _, window, cx| {
                                view.execute_menu(item.command.clone(), window, cx)
                            }),
                        ),
                    )
            }));
        Some(div().child(layout.place(
            menu.bounds.get(),
            matches!(
                menu.kind,
                MenuKind::Track(_)
                    | MenuKind::ObjectTrack(_)
                    | MenuKind::ObjectLinks { .. }
                    | MenuKind::ObjectHero { .. }
                    | MenuKind::NowPlaying
            ),
            (count, rules),
            panel,
            material.is_some() && !self.reduced_motion && !self.fluid_high_contrast(cx),
            cx.listener(|view, _, window, cx| {
                view.dismiss_playlist_overlay(window, cx);
                cx.stop_propagation();
            }),
        )))
    }

    fn playlist_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.playlist.menu.as_ref() else {
            return;
        };
        let items = self.menu_items(menu);
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            self.dismiss_playlist_overlay(window, cx);
        } else if key == "enter" && self.playlist.menu_focus.is_focused(window) {
            let selected = menu.selected.min(items.len().saturating_sub(1));
            if let Some(item) = items.get(selected) {
                self.execute_menu(item.command.clone(), window, cx);
            }
        } else if matches!(key, "up" | "down") {
            self.cycle_playlist_menu(key == "up", window, cx);
        } else if key == "tab" {
            // GPUI owns focus traversal; close the panel instead of leaving it
            // behind when Tab moves focus out.
            self.dismiss_playlist_overlay(window, cx);
            return;
        } else {
            return;
        }
        cx.stop_propagation();
    }

    pub(in crate::ui) fn cycle_playlist_menu(
        &mut self,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.playlist.menu.as_ref() else {
            return false;
        };
        let count = self.menu_items(menu).len();
        if let Some(menu) = self.playlist.menu.as_mut().filter(|_| count > 0) {
            let step = if reverse { count - 1 } else { 1 };
            menu.selected = (menu.selected.min(count - 1) + step) % count;
            window.focus(&self.playlist.menu_focus, cx);
            cx.notify();
        }
        true
    }

    fn menu_items(&self, menu: &Menu) -> Vec<Item> {
        let kind = menu.kind;
        let radio = menu
            .radio
            .as_ref()
            .map(|_| Item::new("Радио по треку", IconName::Radio, Command::TrackRadio));
        let ui = &self.playlist;
        match (kind, &ui.open) {
            (MenuKind::LibraryTrack(_), _) => menu
                .recording
                .iter()
                .flat_map(|entry| {
                    let destination = navigation::track_destination(&entry.playlist, &entry.track)
                        .map(|destination| {
                            Item::new(
                                "Открыть страницу трека",
                                IconName::AudioLines,
                                Command::Navigate(destination),
                            )
                        });
                    let like = Subject::new(&entry.playlist, &entry.track).map(|_| {
                        Item::new(
                            "Куда сохранить лайк",
                            IconName::Heart,
                            Command::LibraryLikeDestinations,
                        )
                    });
                    destination.into_iter().chain(like)
                })
                .collect(),
            (MenuKind::NowPlaying, _) => menu
                .recording
                .iter()
                .flat_map(|entry| {
                    let items = track_links(&entry.playlist, &entry.track).map(|link| {
                        Item::new(link.label, link.glyph, Command::Navigate(link.destination))
                    });
                    let context = navigation::collection(&entry.playlist).map(|target| {
                        Item::new(
                            "Контекст воспроизведения",
                            IconName::ListMusic,
                            Command::Navigate(target),
                        )
                    });
                    let source = navigation::track_target(&entry.playlist, &entry.track)
                        .and_then(|target| target.object.url())
                        .map(|url| {
                            Item::new(
                                "Открыть на площадке",
                                IconName::ExternalLink,
                                Command::OpenSource(url),
                            )
                        });
                    items.chain(context).chain(source)
                })
                .collect(),
            (MenuKind::Import, _) => [
                ("Аудиофайлы", IconName::FileAudio, ImportPaths::Files),
                ("Папка", IconName::FolderOpen, ImportPaths::Folder),
                ("Плейлист M3U", IconName::ListMusic, ImportPaths::M3u),
            ]
            .into_iter()
            .map(|(label, icon, kind)| Item::new(label, icon, Command::Import(kind)))
            .collect(),
            (MenuKind::ObjectLinks { can_search }, _) => self
                .music_link_copy_entries()
                .map(|(title, link)| {
                    Item::new(
                        title,
                        IconName::Copy,
                        Command::CopyMusicLink {
                            platform: link.platform.clone(),
                            url: link.url.clone(),
                        },
                    )
                })
                .chain(can_search.then(|| {
                    Item::new(
                        self.music_platform_search_label(),
                        IconName::Search,
                        Command::ToggleMusicPlatformSearch,
                    )
                    .separated(true)
                }))
                .collect(),
            (MenuKind::ObjectHero { playable }, _) => {
                let pin = self
                    .displayed_music_target()
                    .and_then(crate::ui::workspace::pins::pin_of_object);
                playable
                    .then(|| {
                        Item::new(
                            "Добавить в очередь",
                            IconName::ListEnd,
                            Command::ObjectAppend,
                        )
                    })
                    .into_iter()
                    .chain(self.object_radio_available().then(|| {
                        Item::new(
                            "Радио по исполнителю",
                            IconName::Radio,
                            Command::ObjectRadio,
                        )
                    }))
                    .chain(pin.map(|pin| {
                        let pinned = self.is_pinned(&pin.id);
                        Item::new(
                            if pinned {
                                "Открепить"
                            } else {
                                "Закрепить"
                            },
                            if pinned {
                                IconName::PinOff
                            } else {
                                IconName::Pin
                            },
                            Command::PinObject(pin),
                        )
                    }))
                    .chain(self.music_link_copy_entries().filter(|_| {
                        self.displayed_music_target().is_some_and(|target| {
                            matches!(self.workspace.routes.current(), crate::ui::workspace::Route::Object(route) if route == target)
                        })
                    }).map(|(title, link)| {
                        Item::new(
                            title,
                            IconName::Copy,
                            Command::CopyMusicLink {
                                platform: link.platform.clone(),
                                url: link.url.clone(),
                            },
                        )
                    }))
                    .collect()
            }
            (MenuKind::ObjectTrack(index), _) => {
                let mut items: Vec<_> = radio.into_iter().collect();
                if let Some(entry) = self.object_radio_entry(index) {
                    items.extend(track_links(&entry.playlist, &entry.track).map(|link| {
                        Item::new(link.label, link.glyph, Command::Navigate(link.destination))
                    }));
                }
                items
            }
            (_, None) => radio.into_iter().collect(),
            (MenuKind::Columns { album, date }, Some(_)) => {
                let glass = self.uses_fluid_glass();
                let (show_album, added) = if glass {
                    (
                        self.track_column_visibility.shows(TrackColumn::Album),
                        self.track_column_visibility.shows(TrackColumn::Date),
                    )
                } else {
                    (ui.columns.album, ui.columns.added)
                };
                let mut items = Vec::new();
                if album {
                    items.push(Item::new(
                        if show_album {
                            "Скрыть альбом"
                        } else {
                            "Показать альбом"
                        },
                        IconName::LayoutGrid,
                        Command::Album,
                    ));
                }
                if date {
                    items.push(Item::new(
                        if added {
                            "Скрыть дату"
                        } else {
                            "Показать дату"
                        },
                        IconName::LayoutGrid,
                        Command::Date,
                    ));
                }
                if glass {
                    let separate = !items.is_empty();
                    items.extend(TrackDensity::ALL.into_iter().enumerate().map(
                        |(index, density)| {
                            Item::new(density.label(), IconName::List, Command::Density(density))
                                .separated(index == 0 && separate)
                        },
                    ));
                }
                items
            }
            (MenuKind::Playlist | MenuKind::Track(_), Some(open)) => {
                let playlist = open.playlist();
                let row = if let MenuKind::Track(index) = kind {
                    Some(index)
                } else {
                    None
                };
                let playable = playlist.is_some_and(|playlist| {
                    if let Some(index) = row {
                        playlist
                            .tracks
                            .get(index)
                            .is_some_and(|track| can_resolve(&open.target.key, track))
                    } else {
                        open.visible_audio
                    }
                });
                let mut items = Vec::new();
                if playable {
                    if row.is_some() {
                        items.push(Item::new(
                            "Слушать",
                            IconName::Play,
                            Command::Queue(QueueAction::Play, row),
                        ));
                    }
                    items.push(Item::new(
                        "Играть следующим",
                        IconName::ListStart,
                        Command::Queue(QueueAction::Next, row),
                    ));
                    items.push(Item::new(
                        "В конец очереди",
                        IconName::ListEnd,
                        Command::Queue(QueueAction::End, row),
                    ));
                }
                items.extend(radio);
                if let Some(track) = row.and_then(|index| playlist?.tracks.get(index)) {
                    if let Some(index) = row {
                        items.push(
                            Item::new("Сведения", IconName::Info, Command::Details(index))
                                .separated(!items.is_empty()),
                        );
                    }
                    for (index, link) in track_links(&open.target.key, track).enumerate() {
                        let separate = index == 0 && !items.is_empty();
                        items.push(
                            Item::new(link.label, link.glyph, Command::Navigate(link.destination))
                                .separated(separate),
                        );
                    }
                    if let Some(index) = row
                        && matches!(open.target.key, PlaylistKey::Local(_))
                        && matches!(track.media, Some(Media::File(_)))
                        && !ui.local_busy
                    {
                        items.push(
                            Item::new(
                                "Убрать из плейлиста",
                                IconName::Trash,
                                Command::RemoveLocalTrack(index),
                            )
                            .separated(true),
                        );
                    }
                } else if row.is_none() {
                    if let Some(destination) = navigation::owner(open) {
                        items.push(
                            Item::new(
                                "Профиль автора",
                                IconName::UserRound,
                                Command::Navigate(destination),
                            )
                            .separated(!items.is_empty()),
                        );
                    }
                    if matches!(open.target.key, PlaylistKey::Source { .. })
                        && playlist.is_some()
                        && open.request.is_none()
                    {
                        items.push(
                            Item::new("Обновить", IconName::Repeat, Command::Refresh)
                                .separated(!items.is_empty()),
                        );
                    }
                    if let PlaylistKey::Local(id) = open.target.key
                        && playlist.is_some()
                    {
                        let mut separate = !items.is_empty();
                        if !ui.local_busy && !open.is_empty_local() {
                            items.push(
                                Item::new("Добавить файлы", IconName::Plus, Command::AddFiles(id))
                                    .separated(separate),
                            );
                            separate = false;
                        }
                        items.push(
                            Item::new(
                                "Переименовать плейлист",
                                IconName::Pencil,
                                Command::Edit(Edit::Rename(id)),
                            )
                            .separated(separate),
                        );
                        items.push(Item::new(
                            "Удалить плейлист",
                            IconName::Trash,
                            Command::Edit(Edit::Delete(id)),
                        ));
                    }
                    if let Some(link) = playlist
                        .and_then(|playlist| playlist.link.as_ref())
                        .or(open.target.link.as_ref())
                    {
                        let separate = !items.is_empty();
                        items.push(
                            Item::new(
                                "Копировать ссылку",
                                IconName::Link,
                                Command::CopyLink(link.clone()),
                            )
                            .separated(separate),
                        );
                    }
                }
                items
            }
        }
    }

    fn execute_menu(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let like = if matches!(command, Command::LibraryLikeDestinations) {
            self.playlist
                .menu
                .as_ref()
                .and_then(|menu| menu.recording.as_ref())
                .and_then(|entry| Subject::new(&entry.playlist, &entry.track))
        } else {
            None
        };
        let radio = self
            .playlist
            .menu
            .as_mut()
            .and_then(|menu| menu.radio.take());
        self.dismiss_playlist_overlay(window, cx);
        match command {
            Command::Queue(action, row) => self.playlist_queue(action, row, cx),
            Command::TrackRadio => {
                if let Some(entry) = radio {
                    self.start_track_radio(entry, window, cx);
                }
            }
            Command::Details(index) => self.show_playlist_details(index, window, cx),
            Command::Navigate(destination) => self.follow_playlist_link(destination, window, cx),
            Command::LibraryLikeDestinations => {
                if let Some(subject) = like {
                    self.open_like_destinations(subject, false, window, cx);
                }
            }
            Command::OpenSource(url) => cx.open_url(&url),
            Command::CopyLink(link) => cx.write_to_clipboard(ClipboardItem::new_string(link)),
            Command::CopyMusicLink { platform, url } => self.copy_music_link(platform, url, cx),
            Command::ToggleMusicPlatformSearch => self.toggle_music_platform_search(cx),
            Command::ObjectAppend => self.enqueue_open_object(cx),
            Command::ObjectRadio => self.start_open_object_radio(window, cx),
            Command::PinObject(pin) => self.toggle_pin(pin, cx),
            Command::Refresh => self.refresh_playlist(cx),
            Command::RemoveLocalTrack(index) => self.remove_local_playlist_track(index, cx),
            Command::AddFiles(id) => self.add_files_to_playlist(id, cx),
            Command::Import(kind) => self.choose_playlist_paths(kind, cx),
            Command::Edit(edit) => self.edit_local_playlist(edit, window, cx),
            Command::Density(density) => {
                if let Err(error) = self.choose_track_density(density) {
                    self.playlist.notice = Some(format!("Не удалось сохранить плотность: {error}"));
                }
            }
            Command::Album | Command::Date if self.uses_fluid_glass() => {
                let column = if matches!(command, Command::Album) {
                    TrackColumn::Album
                } else {
                    TrackColumn::Date
                };
                if let Err(error) = self.toggle_track_column(column) {
                    self.playlist.notice = Some(format!("Не удалось сохранить колонки: {error}"));
                }
            }
            Command::Album => self.playlist.columns.album = !self.playlist.columns.album,
            Command::Date => self.playlist.columns.added = !self.playlist.columns.added,
        }
        cx.notify();
    }
}
