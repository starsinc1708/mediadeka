use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
    os::windows::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};
use url::Url;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use zip::ZipArchive;

const CDN: &str = "https://soloist-builds.spotifycdn.com/soloist_release_windows_x86_64.zip";
const ARCHIVE_LIMIT: u64 = 64 * 1024 * 1024;
const EXE_LIMIT: u64 = 128 * 1024 * 1024;
const EXE: &str = "soloist.exe";
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(60);

#[derive(Clone, Debug)]
pub(crate) struct Installed {
    pub executable: PathBuf,
    pub version: String,
}

#[derive(Deserialize, Serialize)]
struct Record {
    sha256: String,
    version: String,
}

pub(crate) fn inspect(root: &Path) -> Result<Option<Installed>, String> {
    let record = match fs::read(root.join("install.json")) {
        Ok(bytes) => serde_json::from_slice::<Record>(&bytes)
            .map_err(|_| "Не читается запись об установке Soloist.")?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failed(error)),
    };
    if record.sha256.len() != 64 || !record.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Не читается контрольная сумма установленного Soloist.".into());
    }
    let executable = root.join("bin").join(record.sha256.as_str()).join(EXE);
    validate_pe(&executable)?;
    if digest(&executable)? != record.sha256 {
        return Err("Контрольная сумма установленного Soloist изменилась.".into());
    }
    signature::verify_spotify(&executable, true)?;
    let version = probe(&executable, &AtomicBool::new(false))?;
    if version != record.version {
        return Err("Версия установленного Soloist изменилась.".into());
    }
    Ok(Some(Installed {
        executable,
        version,
    }))
}

pub(crate) fn install(root: &Path, cancelled: &AtomicBool) -> Result<Installed, String> {
    if !cfg!(target_arch = "x86_64") {
        return Err("Spotify Soloist доступен для Windows x64.".into());
    }
    fs::create_dir_all(root).map_err(failed)?;
    let _lease = install_lock(root, cancelled)?;
    check(cancelled)?;
    if let Ok(Some(installed)) = inspect(root) {
        check(cancelled)?;
        return Ok(installed);
    }
    let bin = root.join("bin");
    fs::create_dir_all(&bin).map_err(failed)?;
    let staging = TempDir::new_in(&bin).map_err(failed)?;
    let mut archive = tempfile::tempfile_in(staging.path()).map_err(failed)?;
    download(&mut archive, cancelled)?;
    let candidate = staging.path().join(EXE);
    extract(archive, &candidate, cancelled)?;
    validate_pe(&candidate)?;
    signature::verify_spotify(&candidate, false)?;
    let version = probe(&candidate, cancelled)?;
    let sha256 = digest(&candidate)?;
    let generation = bin.join(sha256.as_str());
    let executable = generation.join(EXE);
    check(cancelled)?;
    fs::create_dir_all(&generation).map_err(failed)?;
    if !executable.is_file() || digest(&executable)? != sha256 {
        // Candidate and destination are on the same volume. A running old
        // generation keeps its original filename throughout this transaction.
        let mut binary = NamedTempFile::new_in(&generation).map_err(failed)?;
        let source = File::open(&candidate).map_err(failed)?;
        copy_bounded(source, binary.as_file_mut(), EXE_LIMIT, cancelled)?;
        binary.as_file().sync_all().map_err(failed)?;
        binary.persist(&executable).map_err(failed)?;
    }
    check(cancelled)?;
    let mut manifest = NamedTempFile::new_in(root).map_err(failed)?;
    serde_json::to_writer(
        manifest.as_file_mut(),
        &Record {
            sha256,
            version: version.clone(),
        },
    )
    .map_err(failed)?;
    manifest.as_file().sync_all().map_err(failed)?;
    check(cancelled)?;
    manifest
        .persist(root.join("install.json"))
        .map_err(failed)?;
    Ok(Installed {
        executable,
        version,
    })
}

pub(super) fn hidden(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null());
    command
}

fn install_lock(root: &Path, cancelled: &AtomicBool) -> Result<File, String> {
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("install.lock"))
        .map_err(failed)?;
    loop {
        check(cancelled)?;
        match lease.try_lock() {
            Ok(()) => return Ok(lease),
            Err(TryLockError::WouldBlock) => thread::sleep(POLL),
            Err(TryLockError::Error(error)) => return Err(failed(error)),
        }
    }
}

fn download(destination: &mut File, cancelled: &AtomicBool) -> Result<(), String> {
    let mut url = Url::parse(CDN).expect("fixed Spotify CDN URL");
    let http = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_resolve(Some(Duration::from_secs(8)))
        .timeout_per_read(Some(Duration::from_secs(8)))
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .new_agent();
    for _ in 0..=5 {
        check(cancelled)?;
        if !trusted(&url) {
            return Err("CDN Spotify перенаправил загрузку на неподдерживаемый адрес.".into());
        }
        let mut response = http
            .get(url.as_str())
            .call()
            .map_err(|_| "Нет ответа от CDN Spotify.")?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or("CDN Spotify не вернул адрес перенаправления.")?;
            url = url
                .join(location)
                .map_err(|_| "CDN Spotify вернул неверный адрес.")?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!(
                "CDN Spotify вернул HTTP {}.",
                response.status().as_u16()
            ));
        }
        if response
            .body()
            .content_length()
            .is_some_and(|length| length > ARCHIVE_LIMIT)
        {
            return Err("Архив Soloist слишком большой.".into());
        }
        copy_bounded(
            response.body_mut().as_reader(),
            &mut *destination,
            ARCHIVE_LIMIT,
            cancelled,
        )?;
        destination.rewind().map_err(failed)?;
        return Ok(());
    }
    Err("CDN Spotify вернул слишком много перенаправлений.".into())
}

fn trusted(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            host == "spotifycdn.com"
                || host.ends_with(".spotifycdn.com")
                || host == "spotify.com"
                || host.ends_with(".spotify.com")
        })
}

fn extract(archive: File, path: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let mut zip = ZipArchive::new(archive).map_err(|_| "Архив Soloist повреждён.")?;
    let mut found = false;
    for index in 0..zip.len() {
        check(cancelled)?;
        let file = zip
            .by_index(index)
            .map_err(|_| "Не удалось прочитать архив Soloist.")?;
        let name = file
            .enclosed_name()
            .filter(|path| {
                path.components()
                    .all(|component| matches!(component, Component::Normal(_)))
            })
            .ok_or("Небезопасное имя в архиве Soloist.")?;
        if file.is_file()
            && name
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(EXE))
        {
            if found || file.size() == 0 || file.size() > EXE_LIMIT {
                return Err("Архив Soloist содержит неверный исполняемый файл.".into());
            }
            let size = file.size();
            let mut output = File::create(path).map_err(failed)?;
            let copied = copy_bounded(file, &mut output, EXE_LIMIT, cancelled)?;
            if copied != size {
                return Err("Архив Soloist повреждён.".into());
            }
            found = true;
        }
    }
    if !found {
        return Err("В архиве Soloist отсутствует исполняемый файл.".into());
    }
    Ok(())
}

fn copy_bounded(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
    cancelled: &AtomicBool,
) -> Result<u64, String> {
    let mut buffer = vec![0; 64 * 1024];
    let mut total = 0;
    loop {
        check(cancelled)?;
        let count = input.read(&mut buffer).map_err(failed)?;
        if count == 0 {
            return Ok(total);
        }
        total += count as u64;
        if total > limit {
            return Err("Размер файла Soloist превышает ограничение.".into());
        }
        output.write_all(&buffer[..count]).map_err(failed)?;
    }
}

fn validate_pe(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(failed)?;
    let size = file.metadata().map_err(failed)?.len();
    let mut dos = [0; 64];
    file.read_exact(&mut dos).map_err(failed)?;
    let pe_offset = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
    if &dos[..2] != b"MZ" || size > EXE_LIMIT || pe_offset + 26 > size {
        return Err("Soloist не является исполняемым файлом Windows x64.".into());
    }
    file.seek(SeekFrom::Start(pe_offset)).map_err(failed)?;
    let mut pe = [0; 26];
    file.read_exact(&mut pe).map_err(failed)?;
    if &pe[..4] != b"PE\0\0"
        || pe[4..6] != [0x64, 0x86]
        || pe[24..26] != [0x0b, 0x02]
        || pe[23] & 0x20 != 0
    {
        return Err("Soloist не является исполняемым файлом Windows x64.".into());
    }
    Ok(())
}

fn digest(path: &Path) -> Result<String, String> {
    let mut input = File::open(path).map_err(failed)?.take(EXE_LIMIT + 1);
    let mut hash = Sha256::new();
    if io::copy(&mut input, &mut hash).map_err(failed)? > EXE_LIMIT {
        return Err("Размер исполняемого файла Soloist превышает ограничение.".into());
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn probe(path: &Path, cancelled: &AtomicBool) -> Result<String, String> {
    let mut child = hidden(path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Не удалось запустить проверку версии Soloist.")?;
    let until = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => {
                return Err(format!(
                    "Soloist не подтвердил версию. Код {}.",
                    status.code().unwrap_or_default()
                ));
            }
            Ok(None) if !cancelled.load(Ordering::Acquire) && Instant::now() < until => {
                thread::sleep(POLL)
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                check(cancelled)?;
                return Err("Soloist не подтвердил версию вовремя.".into());
            }
        }
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .ok_or("Soloist не вывел версию.")?
        .take(4096)
        .read_to_string(&mut output)
        .map_err(failed)?;
    let mut fields = output.split_whitespace();
    if fields
        .next()
        .is_none_or(|field| !field.eq_ignore_ascii_case("soloist"))
    {
        return Err("Spotify Soloist вывел неожиданный формат версии.".into());
    }
    fields
        .next()
        .filter(|field| {
            field.contains('.') && field.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        })
        .map(str::to_owned)
        .ok_or_else(|| "Spotify Soloist не сообщил номер версии.".into())
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("Установка Soloist отменена.".into())
    } else {
        Ok(())
    }
}

fn failed(error: impl std::fmt::Display) -> String {
    format!("Не удалось подготовить Soloist. {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_https_on_spotify_domains() {
        assert!(trusted(&Url::parse(CDN).unwrap()));
        for value in [
            "http://spotifycdn.com/a",
            "https://notspotify.com/a",
            "https://spotify.com.example/a",
            "https://spotify.com:8443/a",
            "https://test@spotify.com/a",
        ] {
            assert!(!trusted(&Url::parse(value).unwrap()), "{value}");
        }
    }

    #[test]
    #[ignore = "downloads the authentic Windows archive from Spotify CDN"]
    fn live_windows_install_from_spotify_cdn() {
        let profile = tempfile::tempdir().unwrap();
        let retained = std::env::var_os("MEDIADEKA_SOLOIST_TEST_INSTALL_DIR").map(PathBuf::from);
        let root = retained.as_deref().unwrap_or(profile.path());
        assert!(
            inspect(root).unwrap().is_none(),
            "use a fresh install test path"
        );
        let cancelled = AtomicBool::new(false);
        let installed = install(root, &cancelled).expect("download and probe Spotify Soloist");
        assert_eq!(inspect(root).unwrap().unwrap().version, installed.version);
        assert_eq!(
            install(root, &cancelled).unwrap().executable,
            installed.executable
        );
        cancelled.store(true, Ordering::Release);
        assert!(install(root, &cancelled).is_err());
        assert_eq!(
            inspect(root).unwrap().unwrap().executable,
            installed.executable
        );
        println!(
            "Spotify CDN Windows x64 Soloist version={}, size={}",
            installed.version,
            fs::metadata(installed.executable).unwrap().len()
        );
    }

    #[test]
    #[ignore = "requires a retained test installation and a deliberately unreachable HTTPS proxy"]
    fn live_windows_install_offline_keeps_previous_build() {
        let root = PathBuf::from(std::env::var_os("MEDIADEKA_SOLOIST_TEST_INSTALL_DIR").unwrap());
        let failed_profile = tempfile::tempdir().unwrap();
        let cancelled = AtomicBool::new(false);
        let installed = inspect(&root).unwrap().unwrap();
        assert_eq!(
            install(&root, &cancelled).unwrap().executable,
            installed.executable
        );
        let error = install(failed_profile.path(), &cancelled).unwrap_err();
        assert!(error.contains("CDN Spotify"), "{error}");
        assert!(inspect(failed_profile.path()).unwrap().is_none());
        assert!(inspect(&root).unwrap().is_some());
        println!(
            "Offline reuse {} succeeded; clean download failed, no install published: {error}",
            installed.version
        );
    }
}
mod signature;
