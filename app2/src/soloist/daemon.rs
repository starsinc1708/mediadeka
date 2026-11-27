use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions, TryLockError},
    io,
    net::{IpAddr, SocketAddr},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

use crate::playback::probe_managed_soloist;

use super::install::{Installed, hidden};

pub(crate) const DEVICE_NAME: &str = "Mediadeka Soloist";
const START_TIMEOUT: Duration = Duration::from_secs(18);
const POLL: Duration = Duration::from_millis(120);
const LOCAL_ADDRESS: &str = "127.0.0.1:0";

static RUNNING: LazyLock<Mutex<HashMap<PathBuf, Weak<Running>>>> = LazyLock::new(Mutex::default);

#[derive(Clone)]
pub(crate) struct Daemon(Arc<Running>);

struct Running {
    endpoint: SocketAddr,
    owned: Mutex<Option<OwnedDaemon>>,
}

struct OwnedDaemon {
    child: Child,
    _job: Job,
    _lease: File,
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        // Windows Soloist has no shutdown command on its WebSocket. Terminate
        // the Child we started and wait before releasing its profile lease.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Daemon {
    pub(crate) fn start(
        installed: &Installed,
        data: &Path,
        cache: &Path,
        key: &str,
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        check(cancelled)?;
        fs::create_dir_all(data).map_err(failed)?;
        fs::create_dir_all(cache).map_err(failed)?;
        let canonical = fs::canonicalize(data).map_err(failed)?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            check(cancelled)?;
            if let Some(existing) = registered(&canonical)? {
                if probe_managed_soloist(existing.endpoint, cancelled) == Ok(true) {
                    return Ok(Self(existing));
                }
                let mut owned = existing.owned.lock().map_err(failed)?;
                if owned.as_mut().is_some_and(|runtime| {
                    runtime
                        .child
                        .try_wait()
                        .is_ok_and(|status| status.is_some())
                }) {
                    owned.take();
                }
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(data.join("mediadeka-daemon.lock"))
                .map_err(failed)?;
            match file.try_lock() {
                Ok(()) => {
                    check(cancelled)?;
                    for file in ["ws.addr", "ws.port"] {
                        match fs::remove_file(data.join(file)) {
                            Ok(()) => {}
                            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                            Err(error) => return Err(failed(error)),
                        }
                    }
                    let mut child = command(installed, data, cache, key)
                        .args([
                            "--cache-size",
                            "256",
                            "--initial-volume",
                            "0",
                            "--ws",
                            LOCAL_ADDRESS,
                        ])
                        .spawn()
                        .map_err(failed)?;
                    let job = match Job::attach(&child) {
                        Ok(job) => job,
                        Err(error) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(failed(error));
                        }
                    };
                    let mut owned = OwnedDaemon {
                        child,
                        _job: job,
                        _lease: file,
                    };
                    let endpoint = await_ready(&mut owned.child, data, deadline, cancelled)?;
                    return shared(&canonical, endpoint, Some(owned));
                }
                Err(TryLockError::WouldBlock) => {
                    if let Some(endpoint) = authenticated(data, cancelled) {
                        if let Some(existing) = registered(&canonical)? {
                            return Ok(Self(existing));
                        }
                        return shared(&canonical, endpoint, None);
                    }
                }
                Err(TryLockError::Error(error)) => return Err(failed(error)),
            }
            if Instant::now() >= deadline {
                return Err("Локальный процесс Spotify Soloist не подтвердил вход.".into());
            }
            thread::sleep(POLL);
        }
    }

    pub(crate) fn endpoint(&self) -> SocketAddr {
        self.0.endpoint
    }
}

fn await_ready(
    child: &mut Child,
    data: &Path,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<SocketAddr, String> {
    loop {
        check(cancelled)?;
        if let Some(status) = child.try_wait().map_err(failed)? {
            return Err(if status.code() == Some(10) {
                "Срок действия установленной сборки Soloist истёк.".into()
            } else {
                format!(
                    "Spotify Soloist завершился перед подтверждением входа. Код {}.",
                    status.code().unwrap_or_default()
                )
            });
        }
        if let Some(endpoint) = authenticated(data, cancelled) {
            return Ok(endpoint);
        }
        if Instant::now() >= deadline {
            return Err("Локальный процесс Spotify Soloist не подтвердил вход.".into());
        }
        thread::sleep(POLL);
    }
}

fn registered(data: &Path) -> Result<Option<Arc<Running>>, String> {
    Ok(RUNNING
        .lock()
        .map_err(failed)?
        .get(data)
        .and_then(Weak::upgrade))
}

fn shared(data: &Path, endpoint: SocketAddr, owner: Option<OwnedDaemon>) -> Result<Daemon, String> {
    let mut registered = RUNNING.lock().map_err(failed)?;
    if owner.is_none()
        && let Some(existing) = registered.get(data).and_then(Weak::upgrade)
        && existing.endpoint == endpoint
    {
        return Ok(Daemon(existing));
    }
    registered.retain(|_, handle| handle.strong_count() > 0);
    let running = Arc::new(Running {
        endpoint,
        owned: Mutex::new(owner),
    });
    registered.insert(data.to_owned(), Arc::downgrade(&running));
    Ok(Daemon(running))
}

fn authenticated(data: &Path, cancelled: &AtomicBool) -> Option<SocketAddr> {
    let endpoint = endpoint(data).ok()?;
    (probe_managed_soloist(endpoint, cancelled) == Ok(true)).then_some(endpoint)
}

fn endpoint(data: &Path) -> Result<SocketAddr, String> {
    let address = fs::read_to_string(data.join("ws.addr")).map_err(failed)?;
    let address = address.trim().parse::<IpAddr>().map_err(failed)?;
    let port = fs::read_to_string(data.join("ws.port")).map_err(failed)?;
    let port = port.trim().parse::<u16>().map_err(failed)?;
    if !address.is_loopback() || port == 0 {
        return Err("Soloist не вернул локальный адрес.".into());
    }
    Ok(SocketAddr::new(address, port))
}

pub(crate) fn command(installed: &Installed, data: &Path, cache: &Path, key: &str) -> Command {
    let mut command = hidden(&installed.executable);
    command
        .current_dir(data)
        .args(["--device-name", DEVICE_NAME, "--api-key", key])
        .arg("--data-dir")
        .arg(data)
        .arg("--cache-dir")
        .arg(cache)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub(crate) struct Job {
    handle: OwnedHandle,
}

impl Job {
    pub(crate) fn attach(child: &Child) -> io::Result<Self> {
        // Create an unnamed job without security attributes. The returned
        // handle transfers to OwnedHandle before any fallible setup call.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // The structure stays live for SetInformationJobObject and child
        // supplies its original process handle, never a reused PID.
        let set = unsafe {
            SetInformationJobObject(
                job.handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { AssignProcessToJobObject(job.handle.as_raw_handle(), child.as_raw_handle()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("Запуск Spotify Soloist отменён.".into())
    } else {
        Ok(())
    }
}

fn failed(error: impl std::fmt::Display) -> String {
    format!("Не удалось запустить Spotify Soloist. {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_loopback_endpoint_files() {
        let data = tempfile::tempdir().unwrap();
        fs::write(data.path().join("ws.port"), "1234\n").unwrap();
        fs::write(data.path().join("ws.addr"), "192.0.2.1").unwrap();
        assert!(endpoint(data.path()).is_err());
        fs::write(data.path().join("ws.addr"), "127.0.0.1").unwrap();
        assert_eq!(endpoint(data.path()).unwrap().to_string(), "127.0.0.1:1234");
    }

    #[test]
    #[ignore = "runs a genuine Soloist in an empty Windows account directory"]
    fn live_windows_daemon_rejects_unpaired_profile_and_releases_lock() {
        let root = PathBuf::from(std::env::var_os("MEDIADEKA_SOLOIST_TEST_INSTALL_DIR").unwrap());
        let installed = crate::soloist::install::inspect(&root).unwrap().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let data = profile.path().join("data");
        let cache = profile.path().join("cache");
        let cancelled = AtomicBool::new(false);
        let result = Daemon::start(
            &installed,
            &data,
            &cache,
            "mediadeka-diagnostic-no-key",
            &cancelled,
        );
        let error = result.err().expect("empty profile cannot authenticate");
        assert!(error.contains("вход"), "{error}");
        if let Ok(endpoint) = endpoint(&data) {
            assert!(
                std::net::TcpStream::connect_timeout(&endpoint, Duration::from_millis(500))
                    .is_err()
            );
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(data.join("mediadeka-daemon.lock"))
            .unwrap();
        lease.try_lock().expect("managed daemon released its lock");
        println!(
            "Unpaired native Soloist {}: {error} Process and profile lease closed.",
            installed.version
        );
    }
}
