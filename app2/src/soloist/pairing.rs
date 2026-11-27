mod key;
mod process;

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use super::{daemon::Daemon, install::Installed};

const CONFIG: &str = "pairing.json";
const GENERATIONS: &str = "pairings";
const PREFIX: &str = "pair-";
const KEY_FILE: &str = "key.dpapi";
const CONFIG_BYTES: u64 = 8 * 1024;
const KEY_BYTES: usize = 8 * 1024;
const CANCELLED: &str = "Сопряжение Spotify Soloist отменено.";
const PAIRING_IO: &str = "Не удалось сохранить сопряжение Spotify Soloist.";
const INVALID_CONFIG: &str = "Повреждены настройки сопряжения Spotify Soloist.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PairProgress {
    Starting,
    Waiting { port: u16 },
    Verifying,
}

#[derive(Clone)]
pub(crate) struct Pairing {
    pub account: String,
    pub data: PathBuf,
    pub cache: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct Config {
    generation: String,
    account: String,
}

impl Pairing {
    pub(crate) fn configured(root: &Path) -> Result<Option<Self>, String> {
        let manifest = match File::open(root.join(CONFIG)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("Не удалось прочитать настройки Spotify Soloist.".into()),
        };
        let mut bytes = Vec::new();
        manifest
            .take(CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Не удалось прочитать настройки Spotify Soloist.")?;
        if bytes.len() as u64 > CONFIG_BYTES {
            return Err(INVALID_CONFIG.into());
        }
        let config: Config = serde_json::from_slice(&bytes).map_err(|_| INVALID_CONFIG)?;
        let name = config
            .generation
            .strip_prefix(PREFIX)
            .ok_or(INVALID_CONFIG)?;
        if name.is_empty()
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric())
            || config.account.trim().is_empty()
        {
            return Err(INVALID_CONFIG.into());
        }
        let folder = root.join(GENERATIONS).join(config.generation);
        let pairing = Self::new(&folder, &config.account);
        if !pairing.data.is_dir() || !folder.join(KEY_FILE).is_file() {
            return Err(INVALID_CONFIG.into());
        }
        Ok(Some(pairing))
    }

    pub(crate) fn key(&self) -> Result<String, String> {
        let folder = self.data.parent().ok_or(INVALID_CONFIG)?;
        key::load(&folder.join(KEY_FILE))
    }

    fn new(folder: &Path, account: &str) -> Self {
        Self {
            account: account.to_owned(),
            data: folder.join("data"),
            cache: folder.join("cache"),
        }
    }
}

pub(crate) fn pair(
    root: &Path,
    installed: &Installed,
    account: &str,
    key: String,
    cancelled: &AtomicBool,
    progress: impl Fn(PairProgress),
) -> Result<(Pairing, Daemon), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > KEY_BYTES || key.chars().any(char::is_control) {
        return Err("Введите действующий ключ Spotify Soloist.".into());
    }
    if account.trim().is_empty() {
        return Err("Войдите в Spotify в Mediadeka перед сопряжением.".into());
    }
    check_cancelled(cancelled)?;
    let generations = root.join(GENERATIONS);
    fs::create_dir_all(&generations).map_err(|_| PAIRING_IO)?;
    let _lock = lock(root)?;
    let stage = tempfile::Builder::new()
        .prefix(PREFIX)
        .tempdir_in(generations)
        .map_err(|_| PAIRING_IO)?;
    let pairing = Pairing::new(stage.path(), account);
    for directory in [&pairing.data, &pairing.cache] {
        fs::create_dir(directory).map_err(|_| PAIRING_IO)?;
    }

    progress(PairProgress::Starting);
    process::run(installed, &pairing, key, cancelled, &progress)?;
    check_cancelled(cancelled)?;
    progress(PairProgress::Verifying);
    check_paired_account(&pairing.data, account).map_err(SoloistAccountError::message)?;
    key::save(&stage.path().join(KEY_FILE), key)?;
    let daemon = Daemon::start(installed, &pairing.data, &pairing.cache, key, cancelled)?;
    check_paired_account(&pairing.data, account).map_err(SoloistAccountError::message)?;

    let generation = stage
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PAIRING_IO)?;
    publish(root, generation, account, cancelled)?;
    // Keep the generation at its original path. The new daemon already owns this data folder.
    let _ = stage.keep();
    Ok((pairing, daemon))
}

fn lock(root: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("pairing.lock"))
        .map_err(|_| PAIRING_IO)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err("Сопряжение Spotify Soloist уже выполняется.".into()),
        Err(TryLockError::Error(_)) => Err(PAIRING_IO.into()),
    }
}

fn publish(
    root: &Path,
    generation: &str,
    account: &str,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let config = Config {
        generation: generation.to_owned(),
        account: account.to_owned(),
    };
    let mut manifest = tempfile::NamedTempFile::new_in(root).map_err(|_| PAIRING_IO)?;
    let bytes = serde_json::to_vec(&config).map_err(|_| PAIRING_IO)?;
    if bytes.len() as u64 > CONFIG_BYTES {
        return Err(PAIRING_IO.into());
    }
    manifest
        .write_all(&bytes)
        .and_then(|()| manifest.as_file().sync_all())
        .map_err(|_| PAIRING_IO)?;
    check_cancelled(cancelled)?;
    manifest
        .persist(root.join(CONFIG))
        .map_err(|_| PAIRING_IO)?;
    Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

#[cfg(feature = "capture")]
pub(super) fn seed_stale_pairing(root: &Path, account: &str) -> Result<(), String> {
    if root.join(CONFIG).exists()
        || account.is_empty()
        || !account
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Для проверки нужен изолированный профиль Spotify без сопряжения.".into());
    }
    let generations = root.join(GENERATIONS);
    fs::create_dir_all(&generations).map_err(|_| PAIRING_IO)?;
    let folder = tempfile::Builder::new()
        .prefix(PREFIX)
        .tempdir_in(generations)
        .map_err(|_| PAIRING_IO)?;
    let pairing = Pairing::new(folder.path(), account);
    fs::create_dir_all(
        pairing
            .data
            .join("settings/Users")
            .join(format!("{account}-user")),
    )
    .map_err(|_| PAIRING_IO)?;
    key::save(
        &folder.path().join(KEY_FILE),
        "mediadeka-local-startup-test",
    )?;
    let name = folder
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PAIRING_IO)?;
    publish(root, name, account, &AtomicBool::new(false))?;
    let _ = folder.keep();
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SoloistAccountError {
    Unknown,
    Mismatch,
}

impl SoloistAccountError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unknown => {
                "Spotify Soloist не подтвердил, с каким аккаунтом выполнено сопряжение."
            }
            Self::Mismatch => "Устройство Mediadeka Soloist выбрано в другом аккаунте Spotify.",
        }
    }
}

pub(crate) fn check_paired_account(
    profile: &Path,
    account: &str,
) -> Result<(), SoloistAccountError> {
    // Soloist stores the paired username as a percent-encoded directory name.
    let mut found = None;
    for entry in fs::read_dir(profile.join("settings").join("Users"))
        .map_err(|_| SoloistAccountError::Unknown)?
    {
        let entry = entry.map_err(|_| SoloistAccountError::Unknown)?;
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix("-user")) else {
            continue;
        };
        if entry
            .file_type()
            .map_err(|_| SoloistAccountError::Unknown)?
            .is_dir()
        {
            let encoded = format!("name={}", stem.replace('+', "%2B").replace('&', "%26"));
            let decoded = url::form_urlencoded::parse(encoded.as_bytes())
                .next()
                .map(|(_, name)| name.to_lowercase())
                .filter(|name| !name.is_empty() && !name.contains(char::REPLACEMENT_CHARACTER))
                .ok_or(SoloistAccountError::Unknown)?;
            if found.as_ref().is_some_and(|first| *first != decoded) {
                return Err(SoloistAccountError::Unknown);
            }
            found = Some(decoded);
        }
    }
    match found {
        Some(paired) if paired == account.to_lowercase() => Ok(()),
        Some(_) => Err(SoloistAccountError::Mismatch),
        None => Err(SoloistAccountError::Unknown),
    }
}

#[cfg(test)]
mod tests;
