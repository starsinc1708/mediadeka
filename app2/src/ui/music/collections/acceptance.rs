use gpui::{Bounds, Context, Pixels, Window};

use crate::{
    music::{
        ObjectKind, ObjectRef,
        library::{CollectionUpdate, MatchLinks, Scope, source_entries, spotify_album_snapshot},
    },
    playlist::{Media, PlaylistTrack},
    providers::{ProviderId, collections::Collection},
    ui::{
        MainView,
        music::detail::hero_motion::cover_span,
        workspace::{LibraryTab, Page, Route},
    },
};

impl MainView {
    pub fn library_track_geometry_for_capture(
        &self,
        index: usize,
        window: &Window,
    ) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let library = &self.music.collections;
        let row = library
            .index
            .rows
            .get(*library.results.visible.get(index)?)?;
        let entry = row.entry(library.location.source.as_ref());
        Some((
            window.rendered_span(cover_span(("library-row", &entry.key)))?,
            window.rendered_span(cover_span(("library", &entry.key)))?,
        ))
    }

    pub fn show_cached_spotify_albums_for_capture(
        &mut self,
        directory: &std::path::Path,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let provider = ProviderId::Spotify;
        let albums = spotify_album_snapshot(directory)
            .map_err(|error| error.to_string())?
            .ok_or("Снимок альбомов Spotify не найден.")?;
        self.show_music_page(Page::Albums, window, cx);
        self.music.request = None;
        let profile = format!("capture-{}", provider.as_str());
        let entries = source_entries(provider, &profile, albums).into();
        self.receive_library_update(
            LibraryTab::Albums,
            CollectionUpdate::Data {
                provider,
                profile,
                entries,
            },
            cx,
        );
        self.music.collections.rebuild(false);
        cx.notify();
        Ok(())
    }

    pub fn toggle_album_view_for_capture(&mut self, cx: &mut Context<Self>) {
        self.toggle_album_view(cx);
    }

    pub fn sample_library_for_capture(
        &mut self,
        file: &std::path::Path,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let spotify = ProviderId::Spotify;
        let youtube = ProviderId::YouTubeMusic;
        let jellyfin = ProviderId::Jellyfin;
        self.show_combined_home_for_capture(cx);
        self.show_music_page(Page::Tracks, window, cx);
        self.music.request = None;

        for (provider, titles) in [
            (
                spotify,
                ["Огни", "Улица без названия", "Ночью", "Тёплый ветер"],
            ),
            (youtube, ["City Lights", "Рассвет", "Над рекой", "Касание"]),
            (
                jellyfin,
                ["Осенний сад", "К морю", "Северный берег", "Медленный танец"],
            ),
        ] {
            let profile = format!("capture-{}", provider.as_str());
            let tracks = titles
                .into_iter()
                .enumerate()
                .map(|(index, title)| {
                    let id = if provider == spotify {
                        format!("1AbCdEfgHiJkL0mNOpQR{index:02}")
                    } else {
                        format!("xyzabc01_{index:02}")
                    };
                    PlaylistTrack {
                        id: id.clone(),
                        title: title.into(),
                        artist: if index % 2 == 0 {
                            "Фонари"
                        } else {
                            "Море"
                        }
                        .into(),
                        artists: Vec::new(),
                        artist_credits: Vec::new(),
                        album_ref: None,
                        album: Some(
                            if index < 2 {
                                "Дорога домой"
                            } else {
                                "Горизонт"
                            }
                            .into(),
                        ),
                        artwork: None,
                        added: Some(format!(
                            "2026-11-{}",
                            if index == 0 && provider == spotify {
                                24
                            } else if index == 0 && provider == youtube {
                                23
                            } else if provider == youtube {
                                3 + index
                            } else {
                                21 - index
                            }
                        )),
                        duration: Some(160 + index as u32 * 19),
                        media: Some(Media::Provider { provider, id }),
                    }
                })
                .collect();
            let entries = source_entries(provider, &profile, Collection::Liked(tracks)).into();
            self.receive_library_update(
                LibraryTab::Tracks,
                CollectionUpdate::Data {
                    provider,
                    profile,
                    entries,
                },
                cx,
            );
        }
        self.receive_library_update(
            LibraryTab::Tracks,
            CollectionUpdate::Links(MatchLinks {
                native: vec![(
                    ObjectRef {
                        provider: spotify,
                        kind: ObjectKind::Track,
                        id: "1AbCdEfgHiJkL0mNOpQR00".into(),
                    },
                    ObjectRef {
                        provider: youtube,
                        kind: ObjectKind::Track,
                        id: "xyzabc01_00".into(),
                    },
                )],
                ..Default::default()
            }),
            cx,
        );
        let local = ["Первые шаги", "Тихий вечер"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| {
                crate::music::library::Entry::sample_file_for_capture(PlaylistTrack {
                    id: format!("file-{index}"),
                    title: title.into(),
                    artist: "Домашняя запись".into(),
                    artists: Vec::new(),
                    artist_credits: Vec::new(),
                    album_ref: None,
                    album: Some("Кассета".into()),
                    artwork: None,
                    added: Some(format!("2026-11-{}", 22 + index)),
                    duration: Some(230 + index as u32 * 17),
                    media: Some(Media::File(file.into())),
                })
            })
            .collect();
        self.receive_library_update(LibraryTab::Tracks, CollectionUpdate::Local(local), cx);
        self.music.collections.rebuild(false);
        cx.notify();
    }

    pub fn set_library_filters_for_capture(&mut self, cx: &mut Context<Self>) {
        let library = &mut self.music.collections;
        library.location.source = Some(Scope::Local);
        library.location.downloaded = true;
        library.location.sort = crate::music::library::LibrarySort::Title;
        self.music
            .filter
            .update(cx, |input, cx| input.set_text("вечер", cx));
        library.filter_changed("вечер".into());
        cx.notify();
    }

    #[cfg(windows)]
    pub fn sample_cached_track_for_capture(
        &mut self,
        audio: &std::path::Path,
        duration: u32,
        cx: &mut Context<Self>,
    ) -> gpui::Result<()> {
        use crate::{music::library::EntryAction, playback::QueueEntry, storage::Store};

        let provider = ProviderId::Spotify;
        let source = &self.music.collections.sources[&provider];
        let Some(EntryAction::Track(playlist, track)) =
            source.entries.first().map(|entry| &entry.action)
        else {
            return Err(std::io::Error::other("Нет тестовой строки Spotify.").into());
        };
        let mut track = track.clone();
        track.duration = Some(duration);
        let entry = QueueEntry {
            playlist: playlist.clone(),
            playlist_revision: None,
            recommendations: Vec::new(),
            index: 0,
            track,
        };
        let quality = self
            .player
            .read(cx)
            .sound_report()
            .1
            .quality
            .as_ref()
            .ok_or_else(|| std::io::Error::other("Нет данных libmpv для тестового файла."))?;
        let store = Store::open_default()?;
        store.publish_cached_audio(&entry, audio, quality)?;
        let profile = format!("capture-{}", provider.as_str());
        let entries = source_entries(provider, &profile, Collection::Liked(vec![entry.track]));
        self.receive_library_update(
            LibraryTab::Tracks,
            CollectionUpdate::Data {
                provider,
                profile: profile.clone(),
                entries: entries.into(),
            },
            cx,
        );
        let downloads = store.cached_recordings()?;
        self.receive_library_update(
            LibraryTab::Tracks,
            CollectionUpdate::Downloads(downloads),
            cx,
        );
        let library = &mut self.music.collections;
        library.location.source = Some(Scope::Provider { provider, profile });
        library.location.downloaded = true;
        self.music
            .filter
            .update(cx, |input, cx| input.set_text("огни", cx));
        library.location.query = "огни".into();
        library.rebuild(false);
        cx.notify();
        Ok(())
    }

    pub fn check_library_history_for_capture(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.navigate(Route::Library(LibraryTab::Albums), window, cx);
        self.navigate(Route::Home, window, cx);
        self.workspace.routes.step(false);
        self.workspace.routes.step(false);
        self.show_route(window, cx);
    }
}
