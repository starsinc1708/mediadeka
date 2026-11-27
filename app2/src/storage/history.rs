use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{TransactionBehavior, params};

use crate::{
    playback::{SoundDetails, SoundSource, SoundStarted},
    playlist::{PlaylistKey, PlaylistTrack},
    providers::ProviderId,
};

use super::{Result, Store, StoreError, provider_id};

/// The profile keeps the newest entries only, so the journal never grows without a bound.
pub(crate) const LIMIT: usize = 1_000;

pub(super) const MIGRATION: &str = "CREATE TABLE listening_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        started INTEGER NOT NULL CHECK (started >= 0),
        playlist_kind TEXT NOT NULL CHECK (playlist_kind IN ('local', 'source')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        playlist_id TEXT NOT NULL CHECK (length(playlist_id) > 0),
        platform TEXT NOT NULL,
        source_kind TEXT NOT NULL CHECK (source_kind IN ('local', 'soloist', 'stream', 'link')),
        source_provider TEXT NOT NULL,
        track TEXT NOT NULL,
        CHECK ((playlist_kind = 'local' AND provider = '' AND profile_id = '' AND platform = '') OR
               (playlist_kind = 'source' AND provider <> '' AND platform = provider)),
        CHECK ((source_kind = 'stream' AND source_provider <> '') OR
               (source_kind <> 'stream' AND source_provider = ''))
    ) STRICT;
    CREATE INDEX listening_history_started ON listening_history (started DESC, id DESC);";

pub(super) const STATION_MIGRATION: &str = "DROP INDEX listening_history_started;
    ALTER TABLE listening_history RENAME TO listening_history_v11;
    CREATE TABLE listening_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        started INTEGER NOT NULL CHECK (started >= 0),
        playlist_kind TEXT NOT NULL CHECK (playlist_kind IN ('local', 'source', 'station')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        playlist_id TEXT NOT NULL CHECK (length(playlist_id) > 0),
        platform TEXT NOT NULL,
        source_kind TEXT NOT NULL CHECK (source_kind IN ('local', 'soloist', 'stream', 'link')),
        source_provider TEXT NOT NULL,
        track TEXT NOT NULL,
        CHECK ((playlist_kind = 'local' AND provider = '' AND profile_id = '' AND platform = '') OR
               (playlist_kind IN ('source', 'station') AND provider <> '' AND platform = provider)),
        CHECK ((source_kind = 'stream' AND source_provider <> '') OR
               (source_kind <> 'stream' AND source_provider = ''))
    ) STRICT;
    INSERT INTO listening_history (id, started, playlist_kind, provider, profile_id, playlist_id,
                                   platform, source_kind, source_provider, track)
    SELECT id, started, playlist_kind, provider, profile_id, playlist_id,
           platform, source_kind, source_provider, track FROM listening_history_v11;
    DROP TABLE listening_history_v11;
    CREATE INDEX listening_history_started ON listening_history (started DESC, id DESC);";

pub(super) const SOUND_MIGRATION: &str = "DROP INDEX listening_history_started;
    ALTER TABLE listening_history RENAME TO listening_history_v15;
    CREATE TABLE listening_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        started INTEGER NOT NULL CHECK (started >= 0),
        playlist_kind TEXT NOT NULL CHECK (playlist_kind IN ('local', 'source', 'station')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        playlist_id TEXT NOT NULL CHECK (length(playlist_id) > 0),
        platform TEXT NOT NULL,
        source_kind TEXT NOT NULL CHECK (source_kind IN ('local', 'soloist', 'stream', 'link', 'cache', 'download')),
        source_provider TEXT NOT NULL,
        track TEXT NOT NULL,
        sound_details TEXT,
        CHECK ((playlist_kind = 'local' AND provider = '' AND profile_id = '' AND platform = '') OR
               (playlist_kind IN ('source', 'station') AND provider <> '' AND platform = provider)),
        CHECK ((source_kind IN ('stream','download') AND source_provider <> '') OR
               (source_kind NOT IN ('stream','download') AND source_provider = ''))
    ) STRICT;
    INSERT INTO listening_history (id, started, playlist_kind, provider, profile_id, playlist_id,
                                   platform, source_kind, source_provider, track)
    SELECT id, started, playlist_kind, provider, profile_id, playlist_id,
           platform, source_kind, source_provider, track FROM listening_history_v15;
    DROP TABLE listening_history_v15;
    CREATE INDEX listening_history_started ON listening_history (started DESC, id DESC);";

/// The local calendar fields of an entry, computed by SQLite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalTimestamp {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HistoryRecord {
    pub id: i64,
    pub started: i64,
    pub key: PlaylistKey,
    pub track: PlaylistTrack,
    pub platform: Option<ProviderId>,
    pub source: SoundSource,
    pub details: SoundDetails,
    pub time: LocalTimestamp,
}

impl Store {
    pub(crate) fn record_sound(&self, started: &SoundStarted) -> Result<()> {
        self.write_sound(
            &started.entry.playlist,
            &started.entry.track,
            started.source,
            Some(&started.details),
        )
    }

    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn record_history(
        &self,
        key: &PlaylistKey,
        track: &PlaylistTrack,
        source: SoundSource,
    ) -> Result<()> {
        self.write_sound(key, track, source, None)
    }

    /// Record the start of a playable entry. The timestamp is the current
    /// wall clock in whole UTC seconds; the local view is derived on read.
    fn write_sound(
        &self,
        key: &PlaylistKey,
        track: &PlaylistTrack,
        source: SoundSource,
        details: Option<&SoundDetails>,
    ) -> Result<()> {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let started = i64::try_from(started).unwrap_or(i64::MAX);
        let (kind, provider, profile, playlist_id, platform) = key_columns(key)?;
        let (source_kind, source_provider) = source_columns(source);
        let track = serde_json::to_string(track)?;
        let details = details.map(serde_json::to_string).transpose()?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO listening_history
                (started, playlist_kind, provider, profile_id, playlist_id, platform,
                 source_kind, source_provider, track, sound_details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                started,
                kind,
                provider,
                profile,
                playlist_id,
                platform,
                source_kind,
                source_provider,
                track,
                details,
            ],
        )?;
        transaction.execute(
            "DELETE FROM listening_history WHERE id NOT IN (
                SELECT id FROM listening_history ORDER BY started DESC, id DESC LIMIT ?1
             )",
            [LIMIT as i64],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Read the newest entries first, with local calendar fields from SQLite.
    pub(crate) fn listening_history(&self) -> Result<Vec<HistoryRecord>> {
        self.listening_history_after(0)
    }

    pub(crate) fn listening_history_after(&self, id: i64) -> Result<Vec<HistoryRecord>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT playlist_kind, provider, profile_id, playlist_id, platform,
                    source_kind, source_provider, track,
                    CAST(strftime('%Y', started, 'unixepoch', 'localtime') AS INTEGER),
                    CAST(strftime('%m', started, 'unixepoch', 'localtime') AS INTEGER),
                    CAST(strftime('%d', started, 'unixepoch', 'localtime') AS INTEGER),
                    CAST(strftime('%H', started, 'unixepoch', 'localtime') AS INTEGER),
                    CAST(strftime('%M', started, 'unixepoch', 'localtime') AS INTEGER),
                    sound_details, id, started
             FROM listening_history
             WHERE id > ?2
             ORDER BY started DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = statement
            .query_map(params![LIMIT as i64, id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    LocalTimestamp {
                        year: row.get(8)?,
                        month: row.get(9)?,
                        day: row.get(10)?,
                        hour: row.get(11)?,
                        minute: row.get(12)?,
                    },
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, i64>(15)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(
                |(
                    kind,
                    provider,
                    profile,
                    playlist_id,
                    platform,
                    source_kind,
                    source_provider,
                    track,
                    time,
                    details,
                    id,
                    started,
                )| {
                    let key = key_from_columns(&kind, &provider, &profile, &playlist_id)?;
                    let platform = if platform.is_empty() {
                        None
                    } else {
                        Some(provider_id(&platform)?)
                    };
                    let source = source_from_columns(&source_kind, &source_provider)?;
                    let track = serde_json::from_str(&track)?;
                    Ok(HistoryRecord {
                        id,
                        started,
                        key,
                        track,
                        platform,
                        source,
                        details: details
                            .as_deref()
                            .map(serde_json::from_str)
                            .transpose()?
                            .unwrap_or_default(),
                        time,
                    })
                },
            )
            .collect()
    }
}

pub(super) fn key_columns(
    key: &PlaylistKey,
) -> Result<(&'static str, String, String, String, String)> {
    match key {
        PlaylistKey::Local(id) => Ok((
            "local",
            String::new(),
            String::new(),
            id.to_string(),
            String::new(),
        )),
        PlaylistKey::Source {
            provider,
            profile,
            id,
        }
        | PlaylistKey::Station {
            provider,
            profile,
            id,
        } => Ok((
            if matches!(key, PlaylistKey::Station { .. }) {
                "station"
            } else {
                "source"
            },
            provider.as_str().to_owned(),
            profile.clone().unwrap_or_default(),
            id.clone(),
            provider.as_str().to_owned(),
        )),
    }
}

pub(super) fn key_from_columns(
    kind: &str,
    provider: &str,
    profile: &str,
    playlist_id: &str,
) -> Result<PlaylistKey> {
    match kind {
        "local" => Ok(PlaylistKey::Local(
            playlist_id.parse().map_err(|_| StoreError::InvalidData)?,
        )),
        "source" | "station" => {
            let provider = provider_id(provider)?;
            let profile = (!profile.is_empty()).then(|| profile.to_owned());
            let id = playlist_id.to_owned();
            Ok(if kind == "station" {
                PlaylistKey::Station {
                    provider,
                    profile,
                    id,
                }
            } else {
                PlaylistKey::Source {
                    provider,
                    profile,
                    id,
                }
            })
        }
        _ => Err(StoreError::InvalidData),
    }
}

fn source_columns(source: SoundSource) -> (&'static str, String) {
    match source {
        SoundSource::LocalFile => ("local", String::new()),
        SoundSource::Cached => ("cache", String::new()),
        SoundSource::Download(provider) => ("download", provider.as_str().to_owned()),
        SoundSource::Soloist => ("soloist", String::new()),
        SoundSource::Stream(provider) => ("stream", provider.as_str().to_owned()),
        SoundSource::Link => ("link", String::new()),
    }
}

fn source_from_columns(kind: &str, provider: &str) -> Result<SoundSource> {
    match kind {
        "local" => Ok(SoundSource::LocalFile),
        "cache" => Ok(SoundSource::Cached),
        "download" => Ok(SoundSource::Download(provider_id(provider)?)),
        "soloist" => Ok(SoundSource::Soloist),
        "stream" => Ok(SoundSource::Stream(provider_id(provider)?)),
        "link" => Ok(SoundSource::Link),
        _ => Err(StoreError::InvalidData),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::playlist::{Media, PlaylistTrack};

    fn entry(title: &str) -> PlaylistTrack {
        PlaylistTrack {
            id: format!("track-{title}"),
            title: title.into(),
            artist: "Артист".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(120),
            media: Some(Media::File(PathBuf::from("audio.mp3"))),
        }
    }

    #[test]
    fn keeps_newest_entries_in_order_and_trims_the_journal() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let key = PlaylistKey::Local(1);
        for index in 0..LIMIT + 5 {
            store
                .record_history(&key, &entry(&index.to_string()), SoundSource::LocalFile)
                .unwrap();
        }
        let rows = store.listening_history().unwrap();
        assert_eq!(rows.len(), LIMIT);
        assert_eq!(rows[0].track.title, (LIMIT + 4).to_string());
        assert_eq!(rows[LIMIT - 1].track.title, "5");
        assert_eq!(rows[0].key, key);
        assert_eq!(rows[0].platform, None);
        assert_eq!(rows[0].source, SoundSource::LocalFile);
        assert!(rows[0].time.month >= 1);
    }

    #[test]
    fn stores_the_source_platform_and_playlist_key() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let key = PlaylistKey::Source {
            provider: ProviderId::Spotify,
            profile: Some("profile".into()),
            id: "playlist".into(),
        };
        store
            .record_history(&key, &entry("One"), SoundSource::Soloist)
            .unwrap();
        store
            .record_history(
                &key,
                &entry("Two"),
                SoundSource::Stream(ProviderId::YandexMusic),
            )
            .unwrap();
        let rows = store.listening_history().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].track.title, "Two");
        assert_eq!(rows[1].track.title, "One");
        assert_eq!(rows[1].source, SoundSource::Soloist);
        assert_eq!(rows[0].source, SoundSource::Stream(ProviderId::YandexMusic));
        assert_eq!(rows[0].platform, Some(ProviderId::Spotify));
        assert_eq!(rows[0].key, key);

        let station = PlaylistKey::Station {
            provider: ProviderId::YandexMusic,
            profile: Some("profile".into()),
            id: "user:onyourwave".into(),
        };
        let sound = SoundSource::Stream(ProviderId::YandexMusic);
        store
            .record_history(&station, &entry("Wave"), sound)
            .unwrap();
        let rows = store.listening_history().unwrap();
        assert_eq!(rows[0].key, station);
        assert_eq!(rows[0].source, sound);
        assert_eq!(rows[1].key, key);
    }

    #[test]
    fn history_reopens_artist_credits_and_legacy_tracks() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let key = PlaylistKey::Source {
            provider: ProviderId::Spotify,
            profile: None,
            id: "playlist".into(),
        };
        let legacy = entry("Legacy");
        let mut track = entry("WASSUP");
        track.artist = "Joey Valence & Brae, JPEGMAFIA".into();
        track.artists = vec!["Joey Valence & Brae".into(), "JPEGMAFIA".into()];
        track.artist_credits = vec![
            crate::playlist::ArtistCredit::from_catalog(
                "Joey Valence & Brae".into(),
                ProviderId::Spotify,
                Some("37i9dQZF1DXcBWIGoYBM5M".into()),
            ),
            crate::playlist::ArtistCredit::from_catalog(
                "JPEGMAFIA".into(),
                ProviderId::Spotify,
                None,
            ),
        ];
        track.album_ref = crate::music::ObjectRef::native(
            ProviderId::Spotify,
            crate::music::ObjectKind::Album,
            "4aawyAB9vmqN3uQ7FjRGTy".into(),
        );
        track.media = Some(Media::Provider {
            provider: ProviderId::Spotify,
            id: track.id.clone(),
        });
        store
            .record_history(&key, &legacy, SoundSource::Soloist)
            .unwrap();
        store
            .record_history(&key, &track, SoundSource::Soloist)
            .unwrap();
        let legacy_has_no_artist_list: bool = store
            .connect()
            .unwrap()
            .query_row(
                "SELECT json_type(track, '$.artists') IS NULL
                    AND json_type(track, '$.artist_credits') IS NULL
                    AND json_type(track, '$.album_ref') IS NULL
                 FROM listening_history ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(legacy_has_no_artist_list);
        drop(store);

        let store = Store::open(directory.path()).unwrap();
        let rows = store.listening_history().unwrap();
        assert_eq!(rows[0].track, track);
        assert_eq!(rows[1].track, legacy);
    }
}
