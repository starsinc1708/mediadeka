//! Installation, verified pairing and lifetime of the Soloist process owned by Mediadeka.

#[cfg(windows)]
mod daemon;
#[cfg(windows)]
mod install;
#[cfg(windows)]
pub(crate) mod pairing;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(windows)]
use std::{path::Path, sync::Mutex};

#[cfg(windows)]
use crate::storage::Store;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Readiness {
    Missing,
    Installed {
        version: String,
        detail: Option<String>,
    },
    Ready {
        version: String,
    },
}

#[cfg(windows)]
pub(crate) use pairing::PairProgress;

#[cfg(not(windows))]
pub(crate) enum PairProgress {
    Starting,
    Waiting { port: u16 },
    Verifying,
}

const CANCELLED: &str = "Настройка Spotify Soloist отменена.";

#[cfg(windows)]
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

#[cfg(windows)]
struct Running {
    data: PathBuf,
    daemon: daemon::Daemon,
}

/// The normal window owns the configured Soloist for the lifetime of the application.
pub(crate) struct Lifetime {
    active: bool,
}

impl Lifetime {
    pub(crate) fn new(active: bool) -> Self {
        Self { active }
    }
}

impl Drop for Lifetime {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.active {
            let daemon = RUNNING.lock().ok().and_then(|mut running| running.take());
            drop(daemon);
        }
    }
}

#[cfg(windows)]
fn root() -> Result<PathBuf, String> {
    let base = Store::profile_directory().map_err(|error| error.to_string())?;
    std::path::absolute(base.join("soloist")).map_err(|error| error.to_string())
}

#[cfg(all(windows, feature = "capture"))]
pub fn seed_soloist_startup_capture(profile: &Path) -> gpui::Result<()> {
    let installed = profile.join("soloist");
    install::inspect(&installed)
        .map_err(std::io::Error::other)?
        .ok_or_else(|| {
            std::io::Error::other(
                "В тестовом профиле отсутствует подтверждённая установка Soloist.",
            )
        })?;
    let account = Store::open(profile)?
        .preferred_account(crate::providers::ProviderId::Spotify)?
        .ok_or_else(|| {
            std::io::Error::other("В тестовом профиле отсутствует подключение Spotify.")
        })?;
    pairing::seed_stale_pairing(&installed, &account.profile.id).map_err(std::io::Error::other)?;
    Ok(())
}

pub(crate) fn refresh(account: Option<&str>, cancelled: &AtomicBool) -> Result<Readiness, String> {
    check_cancelled(cancelled)?;
    #[cfg(windows)]
    {
        let root = root()?;
        if !root.is_dir() {
            return Ok(Readiness::Missing);
        }
        let account = match account {
            Some(account) => Some(account.to_owned()),
            None => Store::open_default()
                .and_then(|store| store.preferred_account(crate::providers::ProviderId::Spotify))
                .map_err(|error| error.to_string())?
                .map(|account| account.profile.id),
        };
        readiness_at(&root, account.as_deref(), cancelled)
    }
    #[cfg(not(windows))]
    {
        let _ = account;
        Ok(Readiness::Missing)
    }
}

pub(crate) fn install_for_profile(
    account: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Readiness, String> {
    check_cancelled(cancelled)?;
    #[cfg(windows)]
    {
        let root = root()?;
        install::install(&root, cancelled)?;
        readiness_at(&root, account, cancelled)
    }
    #[cfg(not(windows))]
    {
        let _ = account;
        Err("Для этой системы нет сборки Spotify Soloist.".into())
    }
}

pub(crate) fn pair_for_profile(
    account: &str,
    key: String,
    cancelled: &AtomicBool,
    progress: impl Fn(PairProgress),
) -> Result<Readiness, String> {
    check_cancelled(cancelled)?;
    #[cfg(windows)]
    {
        let root = root()?;
        let installed = install::inspect(&root)?.ok_or("Сначала установите Spotify Soloist.")?;
        let (pairing, runtime) =
            pairing::pair(&root, &installed, account, key, cancelled, progress)?;
        keep(pairing.data, runtime)?;
        Ok(Readiness::Ready {
            version: installed.version,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (account, key, progress);
        Err("Для этой системы нет сборки Spotify Soloist.".into())
    }
}

#[cfg(windows)]
fn readiness_at(
    root: &Path,
    account: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Readiness, String> {
    let Some(installed) = install::inspect(root)? else {
        return Ok(Readiness::Missing);
    };
    let pairing = pairing::Pairing::configured(root);
    let detail = match (account, pairing) {
        (Some(account), Ok(Some(paired))) => {
            match connect(&installed, paired, account, cancelled) {
                Ok(_) => {
                    return Ok(Readiness::Ready {
                        version: installed.version,
                    });
                }
                Err(error) => Some(error),
            }
        }
        (_, Err(error)) => Some(error),
        _ => None,
    };
    check_cancelled(cancelled)?;
    Ok(Readiness::Installed {
        version: installed.version,
        detail,
    })
}

pub(crate) fn playback_session(
    account: &str,
    cancelled: &AtomicBool,
) -> Result<Option<(SocketAddr, PathBuf)>, String> {
    check_cancelled(cancelled)?;
    #[cfg(windows)]
    {
        let root = root()?;
        let pairing = pairing::Pairing::configured(&root);
        if let Ok(Some(pair)) = &pairing
            && let Some(endpoint) = reuse(pair, account, cancelled, true)?
        {
            return Ok(Some((endpoint, pair.data.clone())));
        }
        let Some(installed) = install::inspect(&root)? else {
            return Ok(None);
        };
        let pair = pairing?.ok_or("Сопрягите Spotify Soloist в Mediadeka.")?;
        connect(&installed, pair, account, cancelled).map(Some)
    }
    #[cfg(not(windows))]
    {
        let _ = account;
        Ok(None)
    }
}

#[cfg(windows)]
pub(crate) fn warm_playback_session(
    account: &str,
    cancelled: &AtomicBool,
) -> Result<Option<(SocketAddr, PathBuf)>, String> {
    check_cancelled(cancelled)?;
    let Some(pair) = pairing::Pairing::configured(&root()?)? else {
        return Ok(None);
    };
    Ok(reuse(&pair, account, cancelled, false)?.map(|endpoint| (endpoint, pair.data)))
}

#[cfg(windows)]
fn connect(
    installed: &install::Installed,
    pair: pairing::Pairing,
    account: &str,
    cancelled: &AtomicBool,
) -> Result<(SocketAddr, PathBuf), String> {
    validate_account(&pair, account)?;
    let key = pair.key()?;
    let daemon = daemon::Daemon::start(installed, &pair.data, &pair.cache, &key, cancelled)?;
    let endpoint = daemon.endpoint();
    check_cancelled(cancelled)?;
    keep(pair.data.clone(), daemon)?;
    Ok((endpoint, pair.data))
}

#[cfg(windows)]
fn validate_account(pair: &pairing::Pairing, account: &str) -> Result<(), String> {
    if pair.account.to_lowercase() != account.to_lowercase() {
        return Err(
            "Spotify Soloist связан с другим аккаунтом. Выполните сопряжение снова.".into(),
        );
    }
    pairing::check_paired_account(&pair.data, account).map_err(|error| error.message().to_owned())
}

#[cfg(windows)]
fn reuse(
    pair: &pairing::Pairing,
    account: &str,
    cancelled: &AtomicBool,
    remove_stale: bool,
) -> Result<Option<SocketAddr>, String> {
    let existing = RUNNING
        .lock()
        .map_err(|_| PROCESS_STATE)?
        .as_ref()
        .filter(|running| running.data == pair.data)
        .map(|running| running.daemon.clone());
    let Some(existing) = existing else {
        return Ok(None);
    };
    validate_account(pair, account)?;
    let endpoint = existing.endpoint();
    if crate::playback::probe_managed_soloist(endpoint, cancelled) == Ok(true) {
        return Ok(Some(endpoint));
    }
    check_cancelled(cancelled)?;
    if !remove_stale {
        return Ok(None);
    }
    let stale = {
        let mut running = RUNNING.lock().map_err(|_| PROCESS_STATE)?;
        if running.as_ref().is_some_and(|running| {
            running.data == pair.data && running.daemon.endpoint() == endpoint
        }) {
            running.take()
        } else {
            None
        }
    };
    drop(stale);
    Ok(None)
}

#[cfg(windows)]
const PROCESS_STATE: &str = "Не удалось сохранить подключение Spotify Soloist.";

#[cfg(windows)]
fn keep(data: PathBuf, daemon: daemon::Daemon) -> Result<(), String> {
    let previous = RUNNING
        .lock()
        .map_err(|_| PROCESS_STATE)?
        .replace(Running { data, daemon });
    drop(previous);
    Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}
