use gpui::{App, Div, Role, div, prelude::*, px};

use crate::{
    playback::QueueEntry,
    playlist::{PlaylistKey, PlaylistTrack},
    ui::{
        components::track_row::Style, fluid_glass::tokens, music::gallery_track_row,
        panel::queue_row, theme,
    },
};

pub(super) fn gallery(cx: &App) -> Div {
    let style = Style::new(cx, theme::fluid_high_contrast(cx));
    let label = |text| {
        div()
            .text_size(px(tokens::text::CAPTION))
            .text_color(style.secondary())
            .child(text)
    };
    let entries = ["Тихая гавань", "Утренний свет"].map(|title| QueueEntry {
        playlist: PlaylistKey::Local(0),
        playlist_revision: None,
        index: 0,
        track: PlaylistTrack {
            id: title.to_owned(),
            title: title.to_owned(),
            artist: "Пример композиции".to_owned(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: None,
            media: None,
        },
        recommendations: Vec::new(),
    });
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .w_full()
        .max_w(px(360.))
        .child(label("Очередь"))
        .children(entries.iter().enumerate().map(|(index, entry)| {
            queue_row(entry, index, entries.len(), index == 0, false, Some(style))
                .id(("appearance-queue-row", index))
                .accessibility_id(format!("appearance-queue-row-{index}"))
                .tab_stop(false)
                .role(Role::ListItem)
                .cursor_default()
        }))
        .child(label("Строка трека").mt(px(6.)))
        .child(gallery_track_row(&entries[0].track, style))
}
