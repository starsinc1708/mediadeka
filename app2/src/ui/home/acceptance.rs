mod mix;

use gpui::{Context, Window};

use crate::{
    playback::Status,
    playlist::PlaylistKey,
    providers::{
        Capabilities, ErrorKind, Home, HomeItem, HomeSection, HomeSectionPage, HomeSectionStyle,
        Profile, ProviderError, ProviderId,
    },
    state::{ActiveSession, HomeContent, HomeView, Intent, Issue, Screen},
    storage::{AccountSummary, Snapshot},
    ui::workspace::{Page, Target},
};

use super::MainView;

impl MainView {
    pub fn show_paged_shelf_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let provider = ProviderId::Spotify;
        let profile = Profile {
            id: "capture-shelf".into(),
            name: "Тестовый профиль".into(),
        };
        let items: Vec<_> = (1..=31)
            .map(|index| HomeItem {
                id: format!("spotify:playlist:{index:022}"),
                title: format!("Плейлист {index:02}"),
                subtitle: "Подборка Spotify".into(),
                url: None,
                artwork: None,
            })
            .collect();
        let snapshot = Snapshot::received(Home {
            title: "Плейлисты Spotify".into(),
            sections: vec![HomeSection {
                title: "Плейлисты Spotify".into(),
                len: items.len(),
                style: HomeSectionStyle::Collections,
                ..Default::default()
            }],
            items,
        });
        let request = self.state.begin(Intent::Startup);
        self.state.finish(
            &request,
            Screen::Home(HomeView {
                session: ActiveSession::Account {
                    provider,
                    profile: profile.clone(),
                    capabilities: Capabilities {
                        home: true,
                        profile_selection: false,
                        device_code: false,
                    },
                },
                content: HomeContent::Fresh(snapshot),
                accounts: vec![AccountSummary { provider, profile }],
                switch_issue: None,
            }),
        );
        self.show_music_page(Page::Home, window, cx);
        self.filter_home(Some(provider), cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn show_soundcloud_partial_for_capture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider = ProviderId::SoundCloud;
        let profile = Profile {
            id: "capture-soundcloud".into(),
            name: "Тестовый профиль".into(),
        };
        let items = [
            ("1", "Ночная сессия", "Новая рекомендация"),
            ("2", "После полуночи", "Новая рекомендация"),
            ("3", "Городские огни", "Сохранённая публикация"),
            ("4", "Долгая дорога домой", "Сохранённый репост"),
        ]
        .map(|(id, title, subtitle)| HomeItem {
            id: id.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            url: None,
            artwork: None,
        })
        .to_vec();
        let snapshot = Snapshot::received(Home {
            title: "SoundCloud".into(),
            items,
            sections: vec![
                HomeSection {
                    title: "Рекомендации для вас".into(),
                    len: 2,
                    ..Default::default()
                },
                HomeSection {
                    title: "Лента подписок".into(),
                    start: 2,
                    len: 2,
                    page: HomeSectionPage::Activity,
                    ..Default::default()
                },
            ],
        });
        let request = self.state.begin(Intent::Startup);
        self.state.finish(
            &request,
            Screen::Home(HomeView {
                session: ActiveSession::Account {
                    provider,
                    profile: profile.clone(),
                    capabilities: Capabilities {
                        home: true,
                        profile_selection: false,
                        device_code: false,
                    },
                },
                content: HomeContent::Partial {
                    snapshot,
                    issue: Issue::Provider(ProviderError {
                        kind: ErrorKind::Unavailable,
                        message: "Лента подписок временно недоступна.".into(),
                    }),
                },
                accounts: vec![AccountSummary { provider, profile }],
                switch_issue: None,
            }),
        );
        self.filter_home(Some(provider), cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn filter_active_home_for_capture(&mut self, cx: &mut Context<Self>) {
        if let Some((home, _)) = self.state.home_view() {
            self.filter_home(Some(super::FeedSource::active(home).provider), cx);
        }
    }

    pub fn show_combined_home_for_capture(&mut self, cx: &mut Context<Self>) {
        use super::FeedSource;

        let spotify = ProviderId::Spotify;
        let jellyfin = ProviderId::Jellyfin;
        let youtube = ProviderId::YouTubeMusic;
        let yandex = ProviderId::YandexMusic;
        let soundcloud = ProviderId::SoundCloud;
        let make_account = |provider: ProviderId| AccountSummary {
            provider,
            profile: Profile {
                id: format!("capture-{}", provider.as_str()),
                name: "Тестовый профиль".into(),
            },
        };
        let accounts = vec![
            make_account(spotify),
            make_account(jellyfin),
            make_account(youtube),
            make_account(yandex),
            make_account(soundcloud),
        ];
        let profile = accounts[0].profile.clone();
        let spotify_home = sample_collection(spotify, 1, ["Новые релизы", "Снова в наушниках"]);
        let request = self.state.begin(Intent::Startup);
        self.state.finish(
            &request,
            Screen::Home(HomeView {
                session: ActiveSession::Account {
                    provider: spotify,
                    profile,
                    capabilities: Capabilities {
                        home: true,
                        profile_selection: false,
                        device_code: false,
                    },
                },
                content: HomeContent::Fresh(spotify_home),
                accounts: accounts.clone(),
                switch_issue: None,
            }),
        );
        self.home.feeds.set_for_capture(
            FeedSource {
                provider: jellyfin,
                profile: Some(accounts[1].profile.id.clone()),
            },
            HomeContent::Cached {
                snapshot: sample_collection(
                    jellyfin,
                    3,
                    ["Из домашней коллекции", "Последние альбомы"],
                ),
                issue: Issue::Provider(ProviderError {
                    kind: ErrorKind::Network,
                    message: "Сервер недоступен.".into(),
                }),
            },
        );
        self.home.feeds.set_for_capture(
            FeedSource {
                provider: youtube,
                profile: Some(accounts[2].profile.id.clone()),
            },
            HomeContent::Fresh(sample_collection(
                youtube,
                2,
                ["Миксы для вас", "Утренние миксы"],
            )),
        );
        self.home.feeds.set_for_capture(
            FeedSource {
                provider: yandex,
                profile: Some(accounts[3].profile.id.clone()),
            },
            HomeContent::Fresh(sample_collection(
                yandex,
                1,
                ["Новое для тебя", "Твои плейлисты"],
            )),
        );
        self.home.feeds.set_for_capture(
            FeedSource {
                provider: soundcloud,
                profile: Some(accounts[4].profile.id.clone()),
            },
            HomeContent::Fresh(sample_collection(
                soundcloud,
                1,
                ["Рекомендации", "Понравившиеся треки"],
            )),
        );
        self.filter_home(None, cx);
        cx.notify();
    }

    pub fn combined_home_semantics_for_capture(&self) -> serde_json::Value {
        let Some((home, refreshing)) = self.state.home_view() else {
            return serde_json::Value::Null;
        };
        let active = super::FeedSource::active(home);
        let sources = super::FeedSource::available(home);
        let visible = sources.iter()
            .filter(|source| self.home.filter.is_none_or(|provider| source.provider == provider))
            .map(|source| {
                let feed = self.home.feeds.get(source, &active, home, refreshing);
                serde_json::json!({"provider": source.provider.as_str(), "status": feed.status(), "shelves": feed.snapshot().map_or(0, |snapshot| snapshot.home.sections.len()) })
            }).collect::<Vec<_>>();
        serde_json::json!({"active": active.provider.as_str(), "visible": visible})
    }

    pub fn open_home_listening_fixture(
        &mut self,
        id: i64,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.show_combined_home_for_capture(cx);
        self.show_playlist(Target::local(id, "Локальный плейлист".into()), window, cx);
    }

    pub fn return_home_from_listening_fixture(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.show_music_page(Page::Home, window, cx);
    }

    pub fn home_fixture_queue_open(&self) -> bool {
        self.panel_is_open_on_queue()
    }

    pub fn home_for_you_origin_for_capture(&self) -> f32 {
        self.home.for_you_origin.get()
    }

    pub fn pause_home_listening_fixture(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.toggle_pause())
        });
    }

    pub fn seek_home_listening_fixture(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| {
                if let Some(duration) = playback.duration() {
                    playback.seek(duration * 0.42);
                }
            });
        });
    }

    pub fn home_listening_fixture_state(
        &self,
        id: i64,
        cx: &Context<Self>,
    ) -> (Option<u64>, f64, bool, f32, usize, usize) {
        let key = PlaylistKey::Local(id);
        let collection_key = crate::playback::CollectionKey::Playlist(key.clone());
        let resume = self
            .history
            .collections
            .iter()
            .find(|record| record.key == collection_key);
        let position = self.player.read(cx).position();
        let playing = self.player_snapshot.status == Status::Playing;
        let recent = self
            .history
            .rows()
            .iter()
            .filter(|row| row.key == key)
            .count();
        (
            resume.map(|record| record.progress.position_ms),
            position,
            playing,
            resume.map_or(0., |record| record.progress.fraction()),
            recent,
            self.player_snapshot.queue_len,
        )
    }
}

fn sample_collection(provider: ProviderId, received_at_ms: i64, shelves: [&str; 2]) -> Snapshot {
    let mut items = Vec::new();
    let mut sections = Vec::new();
    for title in shelves {
        let start = items.len();
        for track in [
            "Ночной поезд",
            "Первые огни",
            "Последняя остановка",
            "На рассвете",
        ] {
            let id = match provider {
                ProviderId::Spotify => format!("spotify:track:{:022}", items.len() + 1),
                ProviderId::YouTubeMusic if title == "Миксы для вас" => {
                    format!("browse:VLPL{:011}", items.len() + 1)
                }
                ProviderId::YouTubeMusic => format!("video:{:011}", items.len() + 1),
                ProviderId::YandexMusic => format!("track:{}", items.len() + 1),
                ProviderId::SoundCloud => format!("{}", items.len() + 1),
                _ => format!("{:032}", items.len() + 1),
            };
            items.push(HomeItem {
                id,
                title: track.into(),
                subtitle: provider.display_name().into(),
                url: None,
                artwork: None,
            });
        }
        sections.push(HomeSection {
            title: title.into(),
            start,
            len: items.len() - start,
            ..Default::default()
        });
    }
    Snapshot {
        home: Home {
            title: provider.display_name().into(),
            sections,
            items,
        },
        received_at_ms,
    }
}
