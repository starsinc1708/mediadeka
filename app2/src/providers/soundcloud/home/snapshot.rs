use serde::{Deserialize, Serialize};

use crate::providers::{
    ErrorKind, Home, HomeItem, HomeSection, HomeSectionPage, HomeSectionStyle, ProviderError,
    transport::invalid_response,
};

use super::PROVIDER;

type ShelfLoad = Result<Vec<(String, Vec<HomeItem>)>, ProviderError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundCloudShelf {
    Discover,
    Buzzing,
    Activity,
    Recent,
    History,
    Likes,
    Following,
}

impl SoundCloudShelf {
    fn title(self) -> &'static str {
        match self {
            Self::Discover => "Рекомендации",
            Self::Buzzing => "Buzzing",
            Self::Activity => "Лента подписок",
            Self::Recent => "Недавно открытое",
            Self::History => "История прослушивания",
            Self::Likes => "Понравившиеся треки",
            Self::Following => "Подписки",
        }
    }

    fn page(self) -> HomeSectionPage {
        match self {
            Self::Discover | Self::Buzzing => HomeSectionPage::Home,
            Self::Activity => HomeSectionPage::Activity,
            Self::Recent | Self::History | Self::Likes | Self::Following => {
                HomeSectionPage::Library
            }
        }
    }

    pub(super) fn with_items(
        self,
        result: Result<Vec<HomeItem>, ProviderError>,
    ) -> (Self, ShelfLoad) {
        (self, result.map(|items| vec![(self.title().into(), items)]))
    }

    fn from_section(section: &HomeSection) -> Option<Self> {
        section.soundcloud_shelf.or_else(|| match section.page {
            HomeSectionPage::Activity => Some(Self::Activity),
            HomeSectionPage::Home => Some(if section.title == Self::Buzzing.title() {
                Self::Buzzing
            } else {
                Self::Discover
            }),
            HomeSectionPage::Library => [Self::Recent, Self::History, Self::Likes, Self::Following]
                .into_iter()
                .find(|shelf| shelf.title() == section.title),
        })
    }
}

pub(crate) struct SoundCloudUpdate {
    shelves: [(SoundCloudShelf, ShelfLoad); 7],
}

impl SoundCloudUpdate {
    pub(super) fn new(shelves: [(SoundCloudShelf, ShelfLoad); 7]) -> Result<Self, ProviderError> {
        let update = Self { shelves };
        if update.shelves.iter().all(|(_, result)| result.is_err()) {
            return Err(update
                .issue()
                .cloned()
                .unwrap_or_else(|| invalid_response(PROVIDER)));
        }
        Ok(update)
    }

    pub(crate) fn is_partial(&self) -> bool {
        self.shelves.iter().any(|(_, result)| result.is_err())
    }

    pub(crate) fn may_update_cache(&self) -> bool {
        self.shelves
            .iter()
            .all(|(_, result)| result.as_ref().err().is_none_or(can_reuse_cache))
    }

    pub(crate) fn apply(&self, previous: Option<&Home>) -> (Home, Option<ProviderError>) {
        let mut home = Home {
            title: PROVIDER.display_name().into(),
            items: Vec::new(),
            sections: Vec::new(),
        };
        for (shelf, result) in &self.shelves {
            match result {
                Ok(sections) => {
                    for (title, items) in sections {
                        append(&mut home, *shelf, title, items);
                    }
                }
                Err(error) => {
                    if let Some(previous) = previous.filter(|_| can_reuse_cache(error)) {
                        append_previous(&mut home, *shelf, previous);
                    }
                }
            }
        }
        (home, self.issue().cloned())
    }

    fn issue(&self) -> Option<&ProviderError> {
        self.shelves
            .iter()
            .filter_map(|(shelf, result)| Some((*shelf, result.as_ref().err()?)))
            .min_by_key(|(shelf, error)| {
                (
                    !matches!(error.kind, ErrorKind::Expired | ErrorKind::Rejected),
                    can_reuse_cache(error),
                    *shelf != SoundCloudShelf::Activity,
                )
            })
            .map(|(_, error)| error)
    }
}

fn can_reuse_cache(error: &ProviderError) -> bool {
    matches!(
        error.kind,
        ErrorKind::Network | ErrorKind::Unavailable | ErrorKind::InvalidResponse
    )
}

fn append_previous(home: &mut Home, shelf: SoundCloudShelf, previous: &Home) {
    // Snapshots from the first feed implementation contained only the activity items.
    if previous.sections.is_empty() && shelf == SoundCloudShelf::Activity {
        append(home, shelf, &previous.title, &previous.items);
        return;
    }
    for section in &previous.sections {
        if SoundCloudShelf::from_section(section) != Some(shelf) {
            continue;
        }
        if let Some(items) = section
            .start
            .checked_add(section.len)
            .and_then(|end| previous.items.get(section.start..end))
        {
            append(home, shelf, &section.title, items);
        }
    }
}

fn append(home: &mut Home, shelf: SoundCloudShelf, title: &str, items: &[HomeItem]) {
    if items.is_empty() {
        return;
    }
    let style = if items.iter().all(HomeItem::is_artist) {
        HomeSectionStyle::Artists
    } else {
        HomeSectionStyle::Cards
    };
    home.sections.push(HomeSection {
        title: title.into(),
        start: home.items.len(),
        len: items.len(),
        style,
        page: shelf.page(),
        soundcloud_shelf: Some(shelf),
        ..Default::default()
    });
    home.items.extend_from_slice(items);
}

#[cfg(test)]
mod tests {
    use super::{SoundCloudShelf::*, *};

    #[test]
    fn refresh_replaces_successful_shelves_and_retains_only_failed_shelves() {
        let mut previous = Home {
            title: "SoundCloud".into(),
            items: Vec::new(),
            sections: Vec::new(),
        };
        append(&mut previous, Discover, "Buzzing", &[track("old-discover")]);
        append(&mut previous, Buzzing, "Buzzing", &[track("old-editorial")]);
        append(
            &mut previous,
            Activity,
            Activity.title(),
            &[track("old-feed")],
        );
        append(
            &mut previous,
            History,
            History.title(),
            &[track("old-history")],
        );
        previous.sections[2].soundcloud_shelf = None;

        let failure = ProviderError::new(ErrorKind::Network, "offline");
        let update = SoundCloudUpdate::new([
            (Discover, Ok(vec![("Now".into(), vec![track("fresh")])])),
            Buzzing.with_items(Err(failure.clone())),
            Activity.with_items(Err(failure.clone())),
            Recent.with_items(Ok(Vec::new())),
            History.with_items(Ok(Vec::new())),
            Likes.with_items(Ok(Vec::new())),
            Following.with_items(Ok(Vec::new())),
        ])
        .unwrap();

        assert!(update.is_partial());
        assert!(update.may_update_cache());
        let (merged, issue) = update.apply(Some(&previous));
        assert_eq!(issue, Some(failure));
        assert_eq!(
            merged
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["fresh", "old-editorial", "old-feed"]
        );
        assert_eq!(
            merged
                .sections
                .iter()
                .map(|section| (section.soundcloud_shelf, section.start, section.len))
                .collect::<Vec<_>>(),
            [
                (Some(Discover), 0, 1),
                (Some(Buzzing), 1, 1),
                (Some(Activity), 2, 1)
            ]
        );
    }

    #[test]
    fn accepts_the_original_activity_snapshot_for_transient_errors_only() {
        let previous = Home {
            title: Activity.title().into(),
            items: vec![track("old-feed")],
            sections: Vec::new(),
        };
        let update = |kind| {
            SoundCloudUpdate::new([
                (Discover, Ok(vec![("Now".into(), vec![track("fresh")])])),
                Buzzing.with_items(Err(ProviderError::new(ErrorKind::Network, "offline"))),
                Activity.with_items(Err(ProviderError::new(kind, "fixture"))),
                Recent.with_items(Ok(Vec::new())),
                History.with_items(Ok(Vec::new())),
                Likes.with_items(Ok(Vec::new())),
                Following.with_items(Ok(Vec::new())),
            ])
            .unwrap()
        };

        let network = update(ErrorKind::Network);
        let (merged, _) = network.apply(Some(&previous));
        assert_eq!(merged.items.len(), 2);
        assert_eq!(merged.items[1].id, "old-feed");
        assert_eq!(merged.sections[1].page, HomeSectionPage::Activity);

        for kind in [ErrorKind::Expired, ErrorKind::Rejected] {
            let denied = update(kind);
            assert!(!denied.may_update_cache());
            let (fresh, issue) = denied.apply(Some(&previous));
            assert_eq!(fresh.items.len(), 1);
            assert_eq!(fresh.items[0].id, "fresh");
            assert_eq!(issue.unwrap().kind, kind);
        }
    }

    fn track(id: &str) -> HomeItem {
        HomeItem {
            id: id.into(),
            title: id.into(),
            subtitle: String::new(),
            url: None,
            artwork: None,
        }
    }
}
