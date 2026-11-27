use rusqlite::{OptionalExtension, params};

use super::{Result, Store, StoreError};

pub(super) const MIGRATION: &str = "CREATE TABLE right_panel (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        is_open INTEGER NOT NULL CHECK (is_open IN (0, 1)),
        tab TEXT NOT NULL CHECK (tab IN ('queue', 'track'))
    ) STRICT;";

pub(super) const LYRICS_TAB_MIGRATION: &str = "ALTER TABLE right_panel RENAME TO old_right_panel;
    CREATE TABLE right_panel (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        is_open INTEGER NOT NULL CHECK (is_open IN (0, 1)),
        tab TEXT NOT NULL CHECK (tab IN ('queue', 'track', 'lyrics'))
    ) STRICT;
    INSERT INTO right_panel SELECT id, is_open, tab FROM old_right_panel;
    DROP TABLE old_right_panel;";

pub(super) const AUTOPLAY_MIGRATION: &str = "ALTER TABLE right_panel ADD COLUMN autoplay
    INTEGER NOT NULL DEFAULT 0 CHECK (autoplay IN (0, 1));";

pub(super) const WIDTH_MIGRATION: &str = "ALTER TABLE right_panel ADD COLUMN width_dip
    INTEGER CHECK (width_dip BETWEEN 1 AND 65535);";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PanelTab {
    #[default]
    Queue,
    Track,
    Lyrics,
}

impl PanelTab {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Track => "track",
            Self::Lyrics => "lyrics",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "queue" => Ok(Self::Queue),
            "track" => Ok(Self::Track),
            "lyrics" => Ok(Self::Lyrics),
            _ => Err(StoreError::InvalidData),
        }
    }
}

/// The saved right-panel state. Without a row the panel is closed on the queue tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PanelState {
    pub open: bool,
    pub tab: PanelTab,
    pub autoplay: bool,
    pub width_dip: Option<u16>,
}

impl Store {
    pub(crate) fn panel_state(&self) -> Result<PanelState> {
        let saved = self
            .connect()?
            .query_row(
                "SELECT is_open, tab, autoplay, width_dip FROM right_panel WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, Option<u16>>(3)?,
                    ))
                },
            )
            .optional()?;
        match saved {
            Some((open, tab, autoplay, width_dip)) => Ok(PanelState {
                open,
                tab: PanelTab::parse(&tab)?,
                autoplay,
                width_dip,
            }),
            None => Ok(PanelState::default()),
        }
    }

    pub(crate) fn save_panel_state(&self, state: &PanelState) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO right_panel (id, is_open, tab, autoplay, width_dip)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET is_open = excluded.is_open, tab = excluded.tab,
                 autoplay = excluded.autoplay, width_dip = excluded.width_dip",
            params![
                state.open,
                state.tab.as_str(),
                state.autoplay,
                state.width_dip
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MIGRATIONS;

    #[test]
    fn a_profile_without_a_row_opens_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(store.panel_state().unwrap(), PanelState::default());
        assert!(!store.panel_state().unwrap().open);
    }

    #[test]
    fn upgrading_a_profile_keeps_panel_and_autoplay_without_inventing_a_width() {
        let directory = tempfile::tempdir().unwrap();
        let previous_version = MIGRATIONS
            .iter()
            .position(|migration| migration.contains(&WIDTH_MIGRATION))
            .unwrap();
        let store = Store::versioned_fixture(directory.path(), previous_version).unwrap();
        store
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO right_panel (id, is_open, tab, autoplay) VALUES (1, 1, 'lyrics', 1)",
                [],
            )
            .unwrap();
        let migrated = Store::open(directory.path()).unwrap();
        assert_eq!(
            migrated.panel_state().unwrap(),
            PanelState {
                open: true,
                tab: PanelTab::Lyrics,
                autoplay: true,
                width_dip: None,
            }
        );
    }

    #[test]
    fn the_panel_state_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .save_panel_state(&PanelState {
                open: true,
                tab: PanelTab::Lyrics,
                autoplay: true,
                width_dip: Some(376),
            })
            .unwrap();
        let reopened = Store::open(directory.path()).unwrap();
        assert_eq!(
            reopened.panel_state().unwrap(),
            PanelState {
                open: true,
                tab: PanelTab::Lyrics,
                autoplay: true,
                width_dip: Some(376),
            }
        );
        reopened
            .save_panel_state(&PanelState {
                open: false,
                tab: PanelTab::Queue,
                autoplay: false,
                width_dip: None,
            })
            .unwrap();
        assert_eq!(reopened.panel_state().unwrap(), PanelState::default());
        assert_eq!(
            Store::open(directory.path())
                .unwrap()
                .panel_state()
                .unwrap(),
            PanelState::default()
        );
    }
}
