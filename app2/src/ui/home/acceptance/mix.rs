use std::io;

use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::{
    music::sound::recording,
    providers::{ErrorKind, ProviderError},
    state::{ActiveSession, HomeContent, HomeSourceRequest, HomeView, Intent, Screen},
    storage::{Startup, Store},
    ui::home::FeedSource,
};

use super::MainView;

impl MainView {
    pub fn show_saved_home_for_mix_capture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Result<()> {
        let store = Store::open_default()?;
        let accounts = store.accounts()?;
        if accounts.is_empty() {
            return Err(
                io::Error::other("В проверочном профиле нет музыкальных подключений.").into(),
            );
        }
        let session = match store.startup()? {
            Startup::Account(account) => ActiveSession::from(&account),
            Startup::Guest | Startup::Welcome => ActiveSession::Guest,
        };
        let mut home = HomeView {
            session,
            content: HomeContent::Loading,
            accounts,
            switch_issue: None,
        };
        let active = FeedSource::active(&home);
        let sources = FeedSource::available(&home);
        let reader = HomeSourceRequest::default();
        for source in sources {
            let snapshot = reader
                .cached(&store, source.provider, source.profile.as_deref())
                .map_err(|issue| io::Error::other(issue.to_string()))?;
            let content = snapshot.map_or_else(
                || {
                    HomeContent::Empty(
                        ProviderError {
                            kind: ErrorKind::Network,
                            message: "Нет сохранённой ленты.".into(),
                        }
                        .into(),
                    )
                },
                HomeContent::Fresh,
            );
            if source == active {
                home.content = content;
            } else {
                self.home.feeds.set_for_capture(source, content);
            }
        }
        let request = self.state.begin(Intent::Startup);
        self.state.finish(&request, Screen::Home(home));
        self.filter_home(None, cx);
        self.home.mix.saved_only = true;
        window.focus(&self.focus, cx);
        cx.notify();
        Ok(())
    }

    pub fn home_mix_queue_for_capture(&self, cx: &Context<Self>) -> Option<Value> {
        if !self.panel_is_open_on_queue() {
            return None;
        }
        let player = self.player.read(cx);
        let entries = player
            .entries()
            .map(|entry| {
                let origins = entry
                    .recommendations
                    .iter()
                    .map(|origin| {
                        json!({
                            "provider": origin.provider.display_name(),
                            "playlist": origin.playlist,
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "title": entry.track.title,
                    "catalog_id": entry.track.id,
                    "recording": recording(&entry.playlist, &entry.track),
                    "origins": origins,
                })
            })
            .collect::<Vec<_>>();
        Some(json!({"current_index": player.current_index(), "queue": entries}))
    }
}
