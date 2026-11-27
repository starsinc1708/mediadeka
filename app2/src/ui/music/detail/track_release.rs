use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{Context, Div, IntoElement, Pixels, Role, Stateful, Task, div, prelude::*, px};

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef, ScopedPage, entity_relations::read_related_album},
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::ProviderId,
    ui::{
        MainView,
        components::{
            Activate, button, entity_link,
            glass_control::Tone,
            music::time_label,
            scrollbar::{scroll_offset, set_handle_offset},
        },
        music::detail_style::Ink,
        playlist::navigation::{self, Destination},
        workspace::{MusicTarget, Route},
    },
};

use super::{Detail, object_entry, playlist_key};

#[derive(Clone, PartialEq, Eq)]
struct Source {
    album: ObjectRef,
    profile: Option<String>,
    origin: Origin,
}

#[derive(Clone, PartialEq, Eq)]
enum Origin {
    Provider {
        requested: ObjectRef,
        canonical: ObjectRef,
    },
    Local(Media),
}

struct Presentation {
    detail: Detail,
    row: usize,
    reveal: Rc<Cell<bool>>,
    info_open: bool,
}

pub(in crate::ui::music) struct ReleaseTracks {
    source: Source,
    presentation: Option<Presentation>,
    cancelled: Arc<AtomicBool>,
    _job: Task<()>,
}

impl Drop for ReleaseTracks {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl ReleaseTracks {
    fn set_presentation(&mut self, album: Option<(Arc<MusicPage>, usize)>) {
        let Some((page, row)) = album else {
            self.presentation = None;
            return;
        };
        let profile = self.source.profile.clone();
        let mut active = self
            .presentation
            .take()
            .filter(|active| active.row == row)
            .unwrap_or_else(|| Presentation {
                detail: Detail::new(MusicTarget {
                    object: page.reference.clone(),
                    profile: profile.clone(),
                    title: String::new(),
                    artwork: None,
                }),
                row,
                reveal: Rc::new(Cell::new(true)),
                info_open: false,
            });
        active.detail.data = Some(ScopedPage {
            page,
            profile,
            receipt: None,
        });
        self.presentation = Some(active);
    }
}

impl MainView {
    pub(in crate::ui) fn forget_track_release(&mut self) {
        self.music.track_release = None;
    }

    pub(in crate::ui::music) fn refresh_track_release(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.track_release_source() else {
            self.forget_track_release();
            return;
        };
        if self
            .music
            .track_release
            .as_ref()
            .is_some_and(|release| release.source == source)
        {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&cancelled);
        let active = source.clone();
        let job = cx.spawn(async move |view, cx| {
            let worker_source = active.clone();
            let worker_flag = Arc::clone(&stopped);
            let (send, receive) = async_channel::bounded(2);
            let _worker = cx.background_executor().spawn(async move {
                read_related_album(
                    &worker_source.album,
                    worker_source.profile.as_deref(),
                    &worker_flag,
                    |page| {
                        let result = page.and_then(|page| {
                            selected_row(&worker_source, &page).map(|row| (page, row))
                        });
                        let _ = send.try_send(result);
                    },
                );
            });
            while let Ok(result) = receive.recv().await {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                if view
                    .update(cx, |view, cx| {
                        if let Some(release) = &mut view.music.track_release
                            && release.source == active
                        {
                            release.set_presentation(result);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.music.track_release = Some(ReleaseTracks {
            source,
            presentation: None,
            cancelled,
            _job: job,
        });
    }

    fn track_release_source(&self) -> Option<Source> {
        match self.workspace.routes.current() {
            Route::Object(target) if target.object.kind == ObjectKind::Track => {
                let data = self.music.object.as_ref()?.data.as_ref()?;
                let reference = &data.page.reference;
                if reference.kind != ObjectKind::Track
                    || reference.provider != target.object.provider
                {
                    return None;
                }
                let album = data
                    .page
                    .album_ref
                    .as_ref()
                    .or_else(|| data.page.tracks.first()?.album_ref.as_ref())?;
                if album.kind != ObjectKind::Album
                    || !album.is_valid()
                    || album.provider != reference.provider
                {
                    return None;
                }
                Some(Source {
                    album: album.clone(),
                    profile: data.profile.clone(),
                    origin: Origin::Provider {
                        requested: target.object.clone(),
                        canonical: reference.clone(),
                    },
                })
            }
            Route::LocalTrack(target) => {
                let album = target.track.album_ref.as_ref()?;
                if album.kind != ObjectKind::Album || !album.is_valid() {
                    return None;
                }
                let profile = match target.key.connected_source() {
                    Some((provider, profile)) if provider == album.provider => {
                        profile.map(str::to_owned)
                    }
                    Some(_) => return None,
                    None => None,
                };
                Some(Source {
                    album: album.clone(),
                    profile,
                    origin: Origin::Local(target.track.media.clone()?),
                })
            }
            _ => None,
        }
    }

    pub(in crate::ui) fn displayed_music_target(&self) -> Option<&MusicTarget> {
        self.displayed_music_object().map(|detail| &detail.target)
    }

    pub(in crate::ui::music) fn displayed_music_object(&self) -> Option<&Detail> {
        self.release_presentation()
            .map(|album| &album.detail)
            .or(self.music.object.as_ref())
    }

    fn release_presentation(&self) -> Option<&Presentation> {
        self.music.track_release.as_ref()?.presentation.as_ref()
    }

    pub(in crate::ui::music) fn has_selected_release(&self) -> bool {
        self.release_presentation().is_some()
    }

    pub(super) fn displayed_kind_label(
        &self,
        kind: ObjectKind,
        page: Option<&MusicPage>,
    ) -> &'static str {
        if kind == ObjectKind::Album && page.is_some_and(|page| page.single) {
            if self.has_selected_release() && page.is_some_and(|page| page.tracks.len() > 1) {
                "Релиз"
            } else {
                "Сингл"
            }
        } else {
            kind.label()
        }
    }

    pub(in crate::ui::music) fn selected_release_index(&self) -> Option<usize> {
        self.release_presentation().map(|album| album.row)
    }

    pub(in crate::ui::music) fn release_selected_row(
        &self,
        row: Div,
        key: &PlaylistKey,
        track: &PlaylistTrack,
        index: usize,
        header_height: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let album = self.release_presentation().expect("release selection");
        let reveal = Rc::clone(&album.reveal);
        let stopped = Arc::clone(&self.music.track_release.as_ref().unwrap().cancelled);
        let scroll = self.scroll.clone();
        let recording = self
            .music
            .object
            .as_ref()
            .and_then(|detail| detail.data.as_ref());
        let entry = match self.workspace.routes.current() {
            Route::LocalTrack(local) => object_entry(&local.key, 0, &local.track),
            _ => recording
                .and_then(|data| {
                    Some(object_entry(
                        &playlist_key(data),
                        0,
                        data.page.tracks.first()?,
                    ))
                })
                .unwrap_or_else(|| object_entry(key, index, track)),
        };
        let details = album
            .info_open
            .then(|| self.selected_track_details(&entry, cx));
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .flex_wrap()
                    .px(px(10.))
                    .py(px(3.))
                    .child(self.track_lyrics_action(entry, cx))
                    .child(
                        button("release-track-info", "Сведения о треке", 42)
                            .h(px(28.))
                            .px(px(7.))
                            .text_size(px(13.))
                            .when_some(self.glass_controls(cx), |control, material| {
                                material.paint(control, Tone::Quiet)
                            })
                            .child("Сведения о треке")
                            .on_toggle(
                                album.info_open,
                                cx.listener(|view, _, _, cx| {
                                    if let Some(album) = view
                                        .music
                                        .track_release
                                        .as_mut()
                                        .and_then(|release| release.presentation.as_mut())
                                    {
                                        album.info_open = !album.info_open;
                                        album.reveal = Rc::new(Cell::new(true));
                                        cx.notify();
                                    }
                                }),
                            ),
                    ),
            )
            .children(details)
            .on_children_prepainted(move |children, window, cx| {
                if !reveal.get() || stopped.load(Ordering::Relaxed) {
                    return;
                }
                let (Some(row), Some(actions)) = (children.first(), children.get(1)) else {
                    return;
                };
                let viewport = scroll.bounds();
                if viewport.size.height <= Pixels::ZERO {
                    return;
                }
                reveal.set(false);
                let top = viewport.top() + header_height + px(6.);
                let bottom = viewport.bottom() - px(6.);
                let content_bottom = children.get(2).map_or(actions.bottom(), |details| {
                    let visible_height = (bottom - top).max(Pixels::ZERO);
                    details.bottom().min(row.top() + visible_height)
                });
                let distance = if row.top() < top {
                    row.top() - top
                } else if content_bottom > bottom {
                    content_bottom - bottom
                } else {
                    return;
                };
                let offset = scroll_offset(&scroll);
                let scroll = scroll.clone();
                let stopped = Arc::clone(&stopped);
                window.defer(cx, move |window, _| {
                    if !stopped.load(Ordering::Relaxed)
                        && scroll_offset(&scroll) == offset
                        && set_handle_offset(&scroll, offset + distance)
                    {
                        window.refresh();
                    }
                });
            })
            .id("release-selected-track")
            .accessibility_id("release-selected-track")
            .role(Role::Group)
            .aria_label(format!("Выбранный трек · {}", track.title))
            .aria_selected(true)
    }

    fn selected_track_details(
        &self,
        entry: &crate::playback::QueueEntry,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let track = &entry.track;
        let original = self.music.object.as_ref();
        let metadata = original
            .and_then(|detail| detail.data.as_ref())
            .map(|data| &data.page);
        let ink = Ink::new(self.uses_fluid_glass(), self.fluid_high_contrast(cx));
        let facts = metadata
            .and_then(|page| page.version.clone())
            .into_iter()
            .chain(track.duration.map(|time| time_label(f64::from(time))))
            .chain(
                metadata
                    .and_then(|page| page.isrc.as_ref())
                    .map(|value| format!("ISRC · {value}")),
            )
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .id("release-track-metadata")
            .accessibility_id("release-track-metadata")
            .role(Role::Group)
            .aria_label("Сведения о треке")
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .child(entity_link::artist_names(
                div().text_color(ink.secondary),
                "release-track-artist",
                &entry.playlist,
                track,
                43,
                cx,
            ))
            .when(!facts.is_empty(), |body| {
                body.child(div().text_color(ink.secondary).child(facts))
            })
            .children(original.and_then(|detail| self.music_link_actions(detail, &track.title, cx)))
            .children(self.track_credits_block(track, cx))
    }
}

fn selected_row(source: &Source, album: &MusicPage) -> Option<usize> {
    let key = PlaylistKey::Source {
        provider: source.album.provider,
        profile: source.profile.clone(),
        id: format!("album:{}", source.album.id),
    };
    let mut candidates = album.tracks.iter().enumerate().filter(|(_, track)| {
        let destination = navigation::track_destination(&key, track);
        is_origin(source, track, destination.as_ref())
    });
    let (index, _) = candidates.next()?;
    candidates.next().is_none().then_some(index)
}

fn is_origin(source: &Source, track: &PlaylistTrack, destination: Option<&Destination>) -> bool {
    match &source.origin {
        Origin::Provider {
            requested,
            canonical,
        } => {
            matches!(destination, Some(Destination::Object(target)) if [requested, canonical].into_iter()
                .any(|current| same_album_track(&source.album, current, &target.object)))
        }
        Origin::Local(media) => {
            matches!(media, Media::File(_) | Media::Url(_)) && track.media.as_ref() == Some(media)
        }
    }
}

fn same_album_track(album: &ObjectRef, current: &ObjectRef, candidate: &ObjectRef) -> bool {
    if current == candidate {
        return true;
    }
    if album.provider != ProviderId::YandexMusic
        || current.provider != album.provider
        || candidate.provider != album.provider
    {
        return false;
    }
    // Yandex's album response can omit the release suffix carried by its track page.
    // Compare bare recording IDs only when the other row contains this verified album.
    let matches_bare_id = |bare: &str, qualified: &str| {
        bare.bytes().all(|byte| byte.is_ascii_digit())
            && qualified.split_once(':') == Some((bare, album.id.as_str()))
    };
    matches_bare_id(&current.id, &candidate.id) || matches_bare_id(&candidate.id, &current.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yandex_bare_ids_match_only_with_the_validated_release_suffix() {
        let reference =
            |id: &str, kind| ObjectRef::native(ProviderId::YandexMusic, kind, id.into()).unwrap();
        let album = reference("900", ObjectKind::Album);
        let track = |id| reference(id, ObjectKind::Track);
        assert!(same_album_track(&album, &track("31:900"), &track("31")));
        assert!(!same_album_track(&album, &track("31:901"), &track("31")));
        assert!(!same_album_track(
            &album,
            &track("31:900"),
            &track("31:901")
        ));
        assert!(!same_album_track(&album, &track("31:900"), &track("32")));
    }

    #[test]
    fn release_selection_requires_one_exact_occurrence() {
        let album =
            ObjectRef::native(ProviderId::YandexMusic, ObjectKind::Album, "900".into()).unwrap();
        let recording =
            ObjectRef::native(ProviderId::YandexMusic, ObjectKind::Track, "31:900".into()).unwrap();
        let source = Source {
            album: album.clone(),
            profile: None,
            origin: Origin::Provider {
                requested: recording.clone(),
                canonical: recording,
            },
        };
        let track = |id: &str| -> PlaylistTrack {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": "Одно название", "artist": "Один исполнитель"
            }))
            .unwrap()
        };
        let mut page = MusicPage::new(album, "Релиз".into());
        page.tracks = vec![track("31:901"), track("32")];
        assert_eq!(selected_row(&source, &page), None);
        page.tracks.push(track("31"));
        assert_eq!(selected_row(&source, &page), Some(2));
        page.tracks.push(track("31:900"));
        assert_eq!(selected_row(&source, &page), None);
    }
}
