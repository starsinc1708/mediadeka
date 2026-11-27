use rusqlite::{OptionalExtension, params};

use super::{Result, Store, StoreError};
use crate::appearance::{TrackColumnVisibility, TrackDensity};

pub(super) const MIGRATION: &str = "CREATE TABLE collection_layout (
    kind TEXT PRIMARY KEY NOT NULL CHECK (kind IN ('albums', 'playlists')),
    view TEXT NOT NULL CHECK (view IN ('grid', 'list', 'compact')),
    card_edge INTEGER NOT NULL CHECK (card_edge BETWEEN 112 AND 256)
) STRICT;
CREATE TABLE track_density (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    density TEXT NOT NULL CHECK (density IN ('compact', 'standard', 'comfortable'))
) STRICT;";

pub(super) const TRACK_COLUMNS_MIGRATION: &str = "ALTER TABLE track_density
    ADD COLUMN hidden_columns INTEGER NOT NULL DEFAULT 0 CHECK (hidden_columns BETWEEN 0 AND 15);";

#[derive(Clone, Copy)]
pub(crate) enum CollectionKind {
    Albums,
    Playlists,
}

impl CollectionKind {
    const fn key(self) -> &'static str {
        match self {
            Self::Albums => "albums",
            Self::Playlists => "playlists",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CollectionView {
    #[default]
    Grid,
    List,
    Compact,
}

impl CollectionView {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::List => "list",
            Self::Compact => "compact",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "grid" => Some(Self::Grid),
            "list" => Some(Self::List),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CollectionDisplay {
    pub(crate) view: CollectionView,
    pub(crate) card_edge: u16,
}

impl CollectionDisplay {
    pub(crate) const MIN_CARD_EDGE: u16 = 112;
    pub(crate) const MAX_CARD_EDGE: u16 = 256;
    pub(crate) const CARD_EDGE_STEP: u16 = 8;
}

impl Default for CollectionDisplay {
    fn default() -> Self {
        Self {
            view: CollectionView::Grid,
            card_edge: 160,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CollectionDisplayChange {
    View(CollectionView),
    CardEdge(u16),
}

fn display(view: &str, card_edge: u16) -> Result<CollectionDisplay> {
    Ok(CollectionDisplay {
        view: CollectionView::parse(view).ok_or(StoreError::InvalidData)?,
        card_edge,
    })
}

impl Store {
    pub(crate) fn collection_display(&self, kind: CollectionKind) -> Result<CollectionDisplay> {
        let saved = self
            .connect()?
            .query_row(
                "SELECT view, card_edge FROM collection_layout WHERE kind = ?1",
                [kind.key()],
                |row| Ok((row.get::<_, String>(0)?, row.get(1)?)),
            )
            .optional()?;
        match saved {
            Some((view, card_edge)) => display(&view, card_edge),
            None => Ok(CollectionDisplay::default()),
        }
    }

    pub(crate) fn save_collection_display(
        &self,
        kind: CollectionKind,
        change: CollectionDisplayChange,
    ) -> Result<CollectionDisplay> {
        let (view, edge) = match change {
            CollectionDisplayChange::View(view) => (Some(view.as_str()), None),
            CollectionDisplayChange::CardEdge(edge) => (None, Some(edge)),
        };
        let defaults = CollectionDisplay::default();
        let (view, edge) = self.connect()?.query_row(
            "INSERT INTO collection_layout (kind, view, card_edge)
             VALUES (?1, COALESCE(?2, ?4), COALESCE(?3, ?5))
             ON CONFLICT (kind) DO UPDATE SET
                 view = COALESCE(?2, collection_layout.view),
                 card_edge = COALESCE(?3, collection_layout.card_edge)
             RETURNING view, card_edge",
            params![
                kind.key(),
                view,
                edge,
                defaults.view.as_str(),
                defaults.card_edge
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get(1)?)),
        )?;
        display(&view, edge)
    }

    pub(crate) fn track_display(&self) -> Result<(TrackDensity, TrackColumnVisibility)> {
        let saved = self
            .connect()?
            .query_row(
                "SELECT density, hidden_columns FROM track_density WHERE id = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u8>(1)?)),
            )
            .optional()?;
        match saved {
            Some((density, hidden)) => Ok((
                TrackDensity::parse(&density).ok_or(StoreError::InvalidData)?,
                TrackColumnVisibility::parse(hidden).ok_or(StoreError::InvalidData)?,
            )),
            None => Ok(Default::default()),
        }
    }

    pub(crate) fn save_track_density(&self, density: TrackDensity) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO track_density (id, density) VALUES (1, ?1)
             ON CONFLICT (id) DO UPDATE SET density = excluded.density",
            [density.as_str()],
        )?;
        Ok(())
    }

    pub(crate) fn save_track_columns(&self, visibility: TrackColumnVisibility) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO track_density (id, density, hidden_columns) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET hidden_columns = excluded.hidden_columns",
            params![
                TrackDensity::default().as_str(),
                visibility.hidden_columns()
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::TrackColumn;

    #[test]
    fn collection_views_and_track_density_reopen_independently() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(
            store.collection_display(CollectionKind::Albums).unwrap(),
            CollectionDisplay::default()
        );
        let albums = CollectionDisplay {
            view: CollectionView::List,
            card_edge: 144,
        };
        let playlists = CollectionDisplay {
            view: CollectionView::Compact,
            card_edge: 208,
        };
        for (kind, display) in [
            (CollectionKind::Albums, albums),
            (CollectionKind::Playlists, playlists),
        ] {
            store
                .save_collection_display(kind, CollectionDisplayChange::CardEdge(display.card_edge))
                .unwrap();
            let updated = Store::open(directory.path())
                .unwrap()
                .save_collection_display(kind, CollectionDisplayChange::View(display.view))
                .unwrap();
            assert_eq!(updated, display);
        }
        store.save_track_density(TrackDensity::Comfortable).unwrap();
        let visibility = TrackColumnVisibility::default().toggle(TrackColumn::Album);
        store.save_track_columns(visibility).unwrap();
        store.save_track_density(TrackDensity::Compact).unwrap();
        let reopened = Store::open(directory.path()).unwrap();
        assert_eq!(
            reopened.collection_display(CollectionKind::Albums).unwrap(),
            albums
        );
        assert_eq!(
            reopened
                .collection_display(CollectionKind::Playlists)
                .unwrap(),
            playlists
        );
        assert_eq!(
            reopened.track_display().unwrap(),
            (TrackDensity::Compact, visibility)
        );
    }
}
