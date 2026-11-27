use std::{
    env,
    fs::{OpenOptions, TryLockError},
    io,
    net::SocketAddr,
    os::windows::process::CommandExt as _,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::super::{DISCONNECTED, POLL_INTERVAL};
use super::live_endpoint;

const START_GRACE: Duration = Duration::from_secs(10);
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub(super) fn connect(
    root: &Path,
    profile: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<SocketAddr, String> {
    let launcher = root.join("Start-Soloist.ps1");
    if !launcher.is_file()
        || !root.join("soloist.exe").is_file()
        || !root.join("key.dpapi").is_file()
    {
        return Err(DISCONNECTED.into());
    }
    // Only one Mediadeka process starts the paired launcher.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("mediadeka-autostart.lock"))
        .map_err(start_error)?;
    let deadline = Instant::now() + START_GRACE;
    match lock.try_lock() {
        Ok(()) if !cancelled() => {
            if let Some(endpoint) = live_endpoint(profile, cancelled)? {
                return Ok(endpoint);
            }
            launch(root, &launcher, deadline, cancelled).map_err(start_error)?;
        }
        Ok(()) | Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(error)) => return Err(start_error(error)),
    }
    while !cancelled() && Instant::now() < deadline {
        if let Ok(Some(endpoint)) = live_endpoint(profile, cancelled) {
            return Ok(endpoint);
        }
        thread::sleep(POLL_INTERVAL);
    }
    Err(DISCONNECTED.into())
}

fn start_error(error: io::Error) -> String {
    format!("Не удалось запустить Spotify Soloist. {error}")
}

fn launch(
    root: &Path,
    launcher: &Path,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    let windows = env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Не найден каталог Windows."))?;
    let shell = Path::new(&windows).join("System32/WindowsPowerShell/v1.0");
    let mut child = Command::new(shell.join("powershell.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(root)
        // PowerShell 7 hosts can provide a module path incompatible with Windows PowerShell.
        .env("PSModulePath", shell.join("Modules"))
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
        .arg(launcher)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "Скрипт запуска завершился: {status}."
                )))
            };
        }
        if cancelled() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Истекло время запуска Soloist.",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}
