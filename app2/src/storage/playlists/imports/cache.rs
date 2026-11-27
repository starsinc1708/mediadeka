use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};

use super::{Metadata, Result};

pub(super) const MIGRATION: &str = "CREATE TABLE local_tag_cache (
    path TEXT PRIMARY KEY,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    modified TEXT NOT NULL,
    title TEXT,
    artist TEXT,
    album TEXT,
    duration INTEGER CHECK (duration > 0)
) STRICT;";

#[derive(Eq, PartialEq)]
pub(super) struct FileStamp {
    bytes: i64,
    modified: SystemTime,
}

impl FileStamp {
    pub(super) fn read(metadata: &fs::Metadata) -> Option<Self> {
        Some(Self {
            bytes: metadata.len().try_into().ok()?,
            modified: metadata.modified().ok()?,
        })
    }

    fn modified_key(&self) -> String {
        match self.modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos().to_string(),
            Err(before) => format!("-{}", before.duration().as_nanos()),
        }
    }
}

pub(super) struct Record {
    pub path: String,
    pub stamp: FileStamp,
    pub tags: Metadata,
}

pub(super) fn lookup(
    connection: &Connection,
    path: &str,
    stamp: &FileStamp,
) -> Result<Option<Metadata>> {
    Ok(connection
        .prepare_cached(
            "SELECT title, artist, album, duration FROM local_tag_cache
             WHERE path = ?1 AND bytes = ?2 AND modified = ?3",
        )?
        .query_row(params![path, stamp.bytes, stamp.modified_key()], |row| {
            Ok(Metadata {
                title: row.get(0)?,
                artist: row.get(1)?,
                album: row.get(2)?,
                duration: row.get(3)?,
            })
        })
        .optional()?)
}

pub(super) fn write(connection: &Connection, records: &[Record]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut statement = connection.prepare_cached(
        "INSERT INTO local_tag_cache (path, bytes, modified, title, artist, album, duration)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (path) DO UPDATE SET bytes = excluded.bytes, modified = excluded.modified,
                title = excluded.title, artist = excluded.artist, album = excluded.album,
                duration = excluded.duration",
    )?;
    for record in records {
        statement.execute(params![
            record.path,
            record.stamp.bytes,
            record.stamp.modified_key(),
            record.tags.title,
            record.tags.artist,
            record.tags.album,
            record.tags.duration,
        ])?;
    }
    Ok(())
}
