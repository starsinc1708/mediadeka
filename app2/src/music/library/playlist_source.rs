use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use crate::{
    music::{
        ObjectKind,
        page_cache::fresh_snapshot,
        region,
        shared_reads::{SharedReads, cache_cleared, check_cancelled, trace_source},
    },
    playlist::{
        Playlist, PlaylistKey,
        library::{AccessError, ReadError},
    },
    providers::{self, ErrorKind, ProviderError, ProviderId},
    storage::{Account, CatalogSave, MusicSnapshot, Store, StoreError},
};

static READS: SharedReads<Key, SavedPlaylist, PlaylistUpdate> = SharedReads::new();

#[derive(Clone, Debug, Eq, PartialEq)]
struct Key {
    provider: ProviderId,
    account: Option<Account>,
    directory: PathBuf,
    region: String,
    id: String,
    generation: i64,
}

#[derive(Debug)]
pub(crate) struct PlaylistScope {
    key: Key,
    slot: String,
    store: Store,
}

#[derive(Clone)]
pub(crate) struct SavedPlaylist {
    pub(crate) playlist: Arc<Playlist>,
    pub(crate) receipt: PlaylistReceipt,
    pub(crate) encoded_bytes: usize,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct PlaylistReceipt {
    scope: Key,
    pub(crate) saved_at_ms: u64,
}

impl PlaylistReceipt {
    pub(crate) fn generation(&self) -> i64 {
        self.scope.generation
    }
}

pub(crate) enum PlaylistCache {
    Unchanged,
    Changed(SavedPlaylist),
}

#[derive(Clone)]
pub(crate) struct PlaylistUpdate {
    pub(crate) playlist: Arc<Playlist>,
    pub(crate) offset: usize,
    pub(crate) total: Option<usize>,
}

pub(crate) fn playlist_snapshot_fresh(saved_at_ms: u64) -> bool {
    fresh_snapshot(ObjectKind::Playlist, saved_at_ms)
}

impl PlaylistScope {
    pub(crate) fn open(key: &PlaylistKey) -> Result<Self, ReadError> {
        let PlaylistKey::Source {
            provider,
            profile,
            id,
        } = key
        else {
            return Err(ReadError::Provider(ProviderError {
                kind: ErrorKind::InvalidInput,
                message: "Для чтения каталога нужен плейлист площадки.".into(),
            }));
        };
        let store = Store::open_default().map_err(ReadError::Store)?;
        let account = match profile.as_deref() {
            Some(profile) => Some(
                store
                    .account(*provider, profile)
                    .map_err(ReadError::Store)?
                    .ok_or(ReadError::Access(AccessError::Disconnected))?,
            ),
            None => store
                .preferred_account(*provider)
                .map_err(ReadError::Store)?,
        };
        if let Some(account) = &account
            && store.account_revoked(account).map_err(ReadError::Store)?
        {
            return Err(ReadError::Provider(account_changed()));
        }
        Self::for_source(&store, *provider, account, id, &region()).map_err(ReadError::Store)
    }

    pub(in crate::music) fn for_source(
        store: &Store,
        provider: ProviderId,
        account: Option<Account>,
        id: &str,
        region: &str,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            key: Key {
                provider,
                account,
                directory: store.directory().to_owned(),
                region: region.to_owned(),
                id: id.to_owned(),
                generation: store.catalog_generation()?,
            },
            slot: format!("playlist:{id}"),
            store: store.clone(),
        })
    }

    pub(crate) fn cached(&self) -> Result<Option<Playlist>, StoreError> {
        self.store.music_snapshot(
            self.key.provider,
            self.key.account.as_ref(),
            &self.key.region,
            &self.slot,
        )
    }

    pub(crate) fn saved(&self) -> Result<Option<SavedPlaylist>, StoreError> {
        Ok(match self.saved_version(None)? {
            Some(PlaylistCache::Changed(saved)) => Some(saved),
            _ => None,
        })
    }

    pub(crate) fn saved_version(
        &self,
        resident: Option<&PlaylistReceipt>,
    ) -> Result<Option<PlaylistCache>, StoreError> {
        let known = resident
            .filter(|receipt| receipt.scope == self.key)
            .map(|receipt| (receipt.saved_at_ms, receipt.generation()));
        Ok(
            match self.store.versioned_music_snapshot(
                self.key.provider,
                self.key.account.as_ref(),
                &self.key.region,
                &self.slot,
                known,
            )? {
                Some(MusicSnapshot::Unchanged) => Some(PlaylistCache::Unchanged),
                Some(MusicSnapshot::Changed {
                    value,
                    saved_at_ms,
                    encoded_bytes,
                    generation,
                }) => Some(PlaylistCache::Changed(SavedPlaylist {
                    playlist: Arc::new(value),
                    receipt: self.receipt(saved_at_ms, generation),
                    encoded_bytes,
                })),
                None => None,
            },
        )
    }

    fn receipt(&self, saved_at_ms: u64, generation: i64) -> PlaylistReceipt {
        let mut scope = self.key.clone();
        scope.generation = generation;
        PlaylistReceipt { scope, saved_at_ms }
    }

    pub(crate) fn save(&self, playlist: &Playlist) -> Result<bool, StoreError> {
        Ok(matches!(
            self.save_catalog(playlist)?,
            CatalogSave::Stored { .. }
        ))
    }

    fn save_catalog(&self, playlist: &Playlist) -> Result<CatalogSave, StoreError> {
        self.store.save_catalog_snapshot(
            self.key.provider,
            self.key.account.as_ref(),
            &self.key.region,
            &self.slot,
            playlist,
            self.key.generation,
        )
    }

    pub(crate) fn refresh(
        &self,
        force: bool,
        cancelled: &AtomicBool,
    ) -> Result<SavedPlaylist, ReadError> {
        self.refresh_with_progress(force, cancelled, |_| {})
    }

    pub(crate) fn refresh_with_progress(
        &self,
        force: bool,
        cancelled: &AtomicBool,
        notify: impl Fn(&PlaylistUpdate),
    ) -> Result<SavedPlaylist, ReadError> {
        check_cancelled(cancelled).map_err(ReadError::Provider)?;
        if !force
            && let Ok(Some(saved)) = self.saved()
            && playlist_snapshot_fresh(saved.receipt.saved_at_ms)
        {
            return Ok(saved);
        }
        let saved = READS
            .read_with_progress(
                &self.key,
                cancelled,
                |progress| {
                    if self.require_current().is_ok() {
                        notify(progress);
                    }
                },
                |publish| {
                    if !force
                        && let Ok(Some(saved)) = self.saved()
                        && playlist_snapshot_fresh(saved.receipt.saved_at_ms)
                    {
                        // Another operation may have committed before we joined the flight.
                        return Ok(saved);
                    }
                    self.require_current()?;
                    let account = self.key.account.as_ref();
                    let credentials = account
                        .map(|account| {
                            self.store
                                .credentials(account)
                                .map_err(store_error)?
                                .ok_or_else(|| ProviderError {
                                    kind: ErrorKind::Expired,
                                    message: AccessError::MissingCredentials.to_string(),
                                })
                        })
                        .transpose()?;
                    let loaded = trace_source(self.key.provider, cancelled, || {
                        providers::read_playlist_with_progress(
                            self.key.provider,
                            credentials.as_ref(),
                            &self.key.id,
                            cancelled,
                            &mut |page| {
                                check_cancelled(cancelled)?;
                                self.require_current()?;
                                publish(PlaylistUpdate {
                                    playlist: Arc::new(Playlist {
                                        name: page.playlist.name.clone(),
                                        owner: page.playlist.owner.clone(),
                                        artwork: page.playlist.artwork.clone(),
                                        link: page.playlist.link.clone(),
                                        tracks: page.tracks.to_vec(),
                                    }),
                                    offset: page.offset,
                                    total: page.total,
                                });
                                check_cancelled(cancelled)
                            },
                        )
                    });
                    if let (Err(error), Some(account)) = (&loaded, account)
                        && error.kind == ErrorKind::Revoked
                    {
                        let _ = self.store.forget_music_account(account);
                    }
                    let playlist = loaded?;
                    match self.save_catalog(&playlist).map_err(store_error)? {
                        CatalogSave::Stored {
                            saved_at_ms,
                            encoded_bytes,
                        } => Ok(SavedPlaylist {
                            playlist: Arc::new(playlist),
                            receipt: self.receipt(saved_at_ms, self.key.generation),
                            encoded_bytes,
                        }),
                        CatalogSave::AccountChanged => Err(account_changed()),
                        CatalogSave::CacheCleared => Err(cache_cleared()),
                    }
                },
            )
            .map_err(ReadError::Provider)?;
        self.require_current().map_err(ReadError::Provider)?;
        Ok(saved)
    }

    pub(crate) fn require_current(&self) -> Result<(), ProviderError> {
        if self.store.catalog_generation().map_err(store_error)? != self.key.generation {
            return Err(cache_cleared());
        }
        if let Some(account) = self.key.account.as_ref()
            && (self
                .store
                .account(account.provider, &account.profile.id)
                .map_err(store_error)?
                .as_ref()
                != Some(account)
                || self.store.account_revoked(account).map_err(store_error)?)
        {
            return Err(account_changed());
        }
        Ok(())
    }
}

fn account_changed() -> ProviderError {
    ProviderError {
        kind: ErrorKind::Revoked,
        message: AccessError::Disconnected.to_string(),
    }
}

fn store_error(error: StoreError) -> ProviderError {
    ProviderError {
        kind: ErrorKind::InvalidResponse,
        message: error.to_string(),
    }
}
