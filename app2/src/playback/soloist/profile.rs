#[cfg(windows)]
mod startup;

use super::{CONNECT_TIMEOUT, DISCONNECTED, Preparation, wire::Client};
use crate::{playback::worker::Failure, soloist::pairing::check_paired_account, storage::Store};
use std::{
    env, fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const INSTALL_DIRECTORY: &str = "Mediadeka-Soloist";
pub(super) const SIGN_IN_REQUIRED: &str = "Войдите в Spotify в локальном Soloist.";

pub(super) fn prepare(
    account: &str,
    cancelled: &AtomicBool,
    preparation: Preparation,
) -> Result<(SocketAddr, PathBuf), Failure> {
    let managed = match preparation {
        Preparation::Playback => crate::soloist::playback_session(account, cancelled),
        Preparation::Prefetch => crate::soloist::warm_playback_session(account, cancelled),
    }
    .map_err(Failure::SoloistSetup)?;
    if let Some(session) = managed {
        return Ok(session);
    }
    if !Store::uses_default_profile() {
        return Err(Failure::SoloistSetup(DISCONNECTED.into()));
    }
    legacy_session(account, &|| cancelled.load(Ordering::Acquire), preparation)
        .map_err(Failure::SoloistSetup)
}

fn legacy_session(
    account: &str,
    cancelled: &dyn Fn() -> bool,
    preparation: Preparation,
) -> Result<(SocketAddr, PathBuf), String> {
    let root = env::var_os("LOCALAPPDATA")
        .map(|root| Path::new(&root).join(INSTALL_DIRECTORY))
        .filter(|root| root.is_dir())
        .ok_or(DISCONNECTED)?;
    let profile = root.join("data");
    check_paired_account(&profile, account).map_err(|error| error.message())?;
    let endpoint = match live_endpoint(&profile, cancelled)? {
        Some(endpoint) => endpoint,
        #[cfg(windows)]
        None if matches!(preparation, Preparation::Playback) && !cancelled() => {
            startup::connect(&root, &profile, cancelled)?
        }
        None => return Err(DISCONNECTED.into()),
    };
    Ok((endpoint, profile))
}

pub(crate) fn probe_managed_soloist(
    endpoint: SocketAddr,
    cancelled: &AtomicBool,
) -> Result<bool, String> {
    if !endpoint.ip().is_loopback() || endpoint.port() == 0 || cancelled.load(Ordering::Acquire) {
        return Err(DISCONNECTED.into());
    }
    auth_status(&mut Client::connect(endpoint)?, || {
        cancelled.load(Ordering::Acquire)
    })
}

fn live_endpoint(
    profile: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<Option<SocketAddr>, String> {
    let Ok(endpoint) = endpoint(profile) else {
        return Ok(None);
    };
    let Ok(mut client) = Client::connect(endpoint) else {
        return Ok(None);
    };
    if auth_status(&mut client, cancelled)? {
        Ok(Some(endpoint))
    } else {
        Err(SIGN_IN_REQUIRED.into())
    }
}

pub(super) fn auth_status(
    client: &mut Client,
    cancelled: impl Fn() -> bool,
) -> Result<bool, String> {
    client.wait(CONNECT_TIMEOUT, cancelled, |state| {
        state.logged_in.is_some()
    })?;
    client.state.logged_in.ok_or_else(|| DISCONNECTED.into())
}

fn endpoint(profile: &Path) -> Result<SocketAddr, String> {
    let address = fs::read_to_string(profile.join("ws.addr")).map_err(|_| DISCONNECTED)?;
    let address = address.trim().parse::<IpAddr>().map_err(|_| DISCONNECTED)?;
    let port = fs::read_to_string(profile.join("ws.port")).map_err(|_| DISCONNECTED)?;
    let port = port.trim().parse::<u16>().map_err(|_| DISCONNECTED)?;
    if !address.is_loopback() || port == 0 {
        return Err(DISCONNECTED.into());
    }
    Ok(SocketAddr::new(address, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_loopback_connections() {
        let path = tempfile::tempdir().unwrap();
        let address = path.path().join("ws.addr");
        let port = path.path().join("ws.port");
        fs::write(&address, "192.0.2.1").unwrap();
        fs::write(&port, "12000").unwrap();
        assert!(endpoint(path.path()).is_err());
        fs::write(&address, "::1").unwrap();
        assert!(endpoint(path.path()).unwrap().ip().is_loopback());
        fs::write(&port, "0").unwrap();
        assert!(endpoint(path.path()).is_err());
    }

    #[test]
    #[ignore = "Requires the installed Windows Soloist. No playback commands."]
    fn installed_soloist_reports_live_authentication_state() {
        let root = PathBuf::from(env::var_os("LOCALAPPDATA").unwrap()).join(INSTALL_DIRECTORY);
        let profile = root.join("data");
        let endpoint = endpoint(&profile).unwrap();
        let mut client = Client::connect(endpoint).unwrap();
        client
            .wait(CONNECT_TIMEOUT, || false, |state| state.logged_in.is_some())
            .unwrap();
        let authenticated = client.state.logged_in.unwrap();
        if authenticated {
            client
                .wait(CONNECT_TIMEOUT, || false, |state| state.complete)
                .unwrap();
            assert_eq!(live_endpoint(&profile, || false).unwrap(), Some(endpoint));
        } else {
            assert_eq!(
                live_endpoint(&profile, || false).unwrap_err(),
                SIGN_IN_REQUIRED
            );
        }
        println!("Installed Soloist authenticated: {authenticated}");
    }
}
