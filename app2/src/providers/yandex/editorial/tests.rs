use super::*;
use serde_json::json;

const ARTWORK: &str = "avatars.yandex.net/get-music-content/88/art/%%";
const PLAYLIST: &str = "12345678-90ab-cdef-1234-567890abcdef";

fn release(id: u64, title: &str) -> Value {
    json!({
        "album": {"id":id,"title":title,"albumType":"single","cover":{"uri":ARTWORK}},
        "artists":[{"id":9,"name":"Автор"}],
        "releaseDate":"2026-11-19T21:00:00Z",
        "cover":{"uri":"avatars.yandex.net/get-music-content/88/artist/%%"}
    })
}

fn station(id: &str) -> Value {
    json!({"stationId":id,"title":"Волна","header":"По жанру","compactImageUrl":ARTWORK})
}

fn test_home() -> Home {
    Home {
        title: "Яндекс Музыка".into(),
        items: Vec::new(),
        sections: Vec::new(),
    }
}

#[test]
fn maps_six_live_shapes_and_preserves_shared_tab_indices() {
    let answer = [
        json!({"inStyleTabs":[
            {"title":"Автор","cover":{"uri":ARTWORK},"items":[release(1,"Первый")]},
            {"title":"Другой автор","items":[release(2,"Второй")]}
        ]}),
        json!({"waves":[
            {"title":"топ","items":[station("genre:pop"),station("mood:calm")]},
            {"title":"по жанру","items":[station("genre:pop"),station("genre:rap")]}
        ]}),
        json!({"newReleases":[release(3,"Рекомендация")]}),
        json!({"newReleases":[release(4,"Тренд")]}),
        json!({"items":[{"data":{"id":"autumn","title":"Осенняя","covers":[{"uri":ARTWORK}],
            "action":{"weblink":"/tag/autumn"}}}]}),
        json!({"items":[{"data":{"playlist":{"playlistUuid":PLAYLIST,"title":"Плейлист","cover":{"uri":ARTWORK}},
            "likesCount":123456}}]}),
        json!({"items":[{"data":{"playlist":{"playlistUuid":PLAYLIST,"title":"Новинки","cover":{"uri":ARTWORK}},
            "likesCount":255}}]}),
    ];
    let mut home = test_home();
    let editorial = Editorial::parse(&answer);
    assert!(editorial.has_waves());
    editorial.append(&mut home);
    assert_eq!(
        home.sections
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>(),
        [
            "В стиле",
            "Волна открытий",
            "Новые релизы",
            "Подборки музыки",
            "Вы могли пропустить",
            "Больше новой музыки"
        ]
    );
    let style = &home.sections[0];
    assert_ne!(style.tabs[0].indices, style.tabs[1].indices);
    assert!(style.tabs[0].artwork.is_some());
    let waves = &home.sections[1];
    assert_eq!(waves.len, 3);
    assert_eq!(waves.tabs[0].indices[0], waves.tabs[1].indices[0]);
    assert!(home.items[waves.tabs[0].indices[0]].url.is_none());
    let releases = &home.sections[2];
    assert_eq!(releases.style, HomeSectionStyle::Releases);
    assert_eq!(releases.tabs[1].title, "Тренды");
    let recommendation = &home.items[releases.tabs[0].indices[0]];
    assert_eq!(recommendation.subtitle, "Автор · Сингл · 20 ноября");
    assert!(
        recommendation
            .artwork
            .as_deref()
            .unwrap()
            .ends_with("/artist/400x400")
    );
    let collection = &home.sections[3];
    assert_eq!(collection.style, HomeSectionStyle::Collections);
    assert_eq!(
        home.items[collection.start].url.as_deref(),
        Some("https://music.yandex.ru/tag/autumn")
    );
    assert_eq!(home.items[home.sections[4].start].subtitle, "♡ 123 456");
    assert_eq!(
        home.items[home.sections[5].start].url,
        Some(format!("{SITE}/playlists/{PLAYLIST}"))
    );
}

#[test]
fn does_not_invent_missing_data_or_accept_an_external_collection_link() {
    let mut answers = std::array::from_fn(|_| Value::Null);
    answers[4] = json!({"items":[{"data":{"id":"autumn","title":"Подмена","action":{
        "weblink":"//elsewhere.invalid/tag/autumn"}}}]});
    answers[3] = json!({"newReleases":[release(3,"Тренд")]});
    let mut home = test_home();
    let editorial = Editorial::parse(&answers);
    assert!(!editorial.has_waves());
    editorial.append(&mut home);
    assert_eq!(home.sections.len(), 1);
    assert_eq!(home.sections[0].tabs[0].title, "Тренды");
    assert_eq!(home.items.len(), 1);
}
