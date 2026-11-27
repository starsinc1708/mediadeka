use super::*;

const TEST_ACCOUNT: &str = "mediadeka-pairing-check";
const TEST_KEY: &str = "00000000000000000000000000000000";

#[test]
fn rejects_mismatched_and_ambiguous_pairings() {
    let stage = tempfile::tempdir().unwrap();
    let users = stage.path().join("settings/Users");
    fs::create_dir_all(users.join("s%C3%A9ance%2Bmix-user")).unwrap();
    assert_eq!(check_paired_account(stage.path(), "Séance+Mix"), Ok(()));
    assert_eq!(
        check_paired_account(stage.path(), "another"),
        Err(SoloistAccountError::Mismatch)
    );
    fs::create_dir(users.join("different-user")).unwrap();
    assert_eq!(
        check_paired_account(stage.path(), "Séance+Mix"),
        Err(SoloistAccountError::Unknown)
    );
}

#[test]
fn rejects_outside_generations() {
    let root = tempfile::tempdir().unwrap();
    assert!(Pairing::configured(root.path()).unwrap().is_none());
    fs::write(
        root.path().join(CONFIG),
        br#"{"generation":"pair-../outside","account":"another"}"#,
    )
    .unwrap();
    assert!(Pairing::configured(root.path()).is_err());
}

#[cfg(windows)]
#[test]
fn dpapi_keeps_the_key_protected_and_cancellation_preserves_the_pairing() {
    let root = tempfile::tempdir().unwrap();
    let previous = configured_fixture(root.path());
    let cipher = fs::read(previous.data.parent().unwrap().join(KEY_FILE)).unwrap();
    assert!(
        !cipher
            .windows(TEST_KEY.len())
            .any(|part| part == TEST_KEY.as_bytes())
    );
    assert!(previous.key().unwrap() == TEST_KEY);
    let manifest = fs::read(root.path().join(CONFIG)).unwrap();
    let stop = AtomicBool::new(true);
    assert!(publish(root.path(), "pair-abandoned", "another", &stop).is_err());
    assert_eq!(fs::read(root.path().join(CONFIG)).unwrap(), manifest);
    let loaded = Pairing::configured(root.path()).unwrap().unwrap();
    assert_eq!(loaded.data, previous.data);
    assert!(loaded.key().unwrap() == TEST_KEY);
}

#[cfg(windows)]
fn configured_fixture(root: &Path) -> Pairing {
    let generation = "pair-previous";
    let directory = root.join(GENERATIONS).join(generation);
    let previous = Pairing::new(&directory, TEST_ACCOUNT);
    fs::create_dir_all(&previous.data).unwrap();
    key::save(&directory.join(KEY_FILE), TEST_KEY).unwrap();
    publish(root, generation, TEST_ACCOUNT, &AtomicBool::new(false)).unwrap();
    previous
}

#[cfg(windows)]
#[test]
#[ignore = "Requires MEDIADEKA_TEST_SOLOIST_EXE. Advertises a disposable device and cancels before sign-in."]
fn windows_real_pairing_listener_starts_and_cancels() {
    use std::{
        cell::Cell,
        env,
        net::{SocketAddr, TcpStream},
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    let binary =
        env::var_os("MEDIADEKA_TEST_SOLOIST_EXE").expect("path to the real Windows Soloist");
    let installed = Installed {
        executable: PathBuf::from(binary),
        version: "pairing-check".into(),
    };
    let root = tempfile::tempdir().unwrap();
    let previous = configured_fixture(root.path());
    let manifest = fs::read(root.path().join(CONFIG)).unwrap();
    let abort = Arc::new(AtomicBool::new(false));
    let watchdog_abort = abort.clone();
    let (done, wait) = mpsc::channel();
    let watchdog = thread::spawn(move || {
        if wait.recv_timeout(Duration::from_secs(25)).is_err() {
            watchdog_abort.store(true, Ordering::Release);
        }
    });
    let listener = Cell::new(None::<SocketAddr>);
    let connected = Cell::new(false);
    let result = pair(
        root.path(),
        &installed,
        TEST_ACCOUNT,
        TEST_KEY.into(),
        &abort,
        |progress| {
            if let PairProgress::Waiting { port } = progress {
                let address = SocketAddr::from(([127, 0, 0, 1], port));
                connected
                    .set(TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok());
                listener.set(Some(address));
                abort.store(true, Ordering::Release);
            }
        },
    );
    let _ = done.send(());
    watchdog.join().unwrap();
    assert_eq!(result.err().as_deref(), Some(CANCELLED));
    let address = listener
        .get()
        .expect("the real Soloist must advertise a listener");
    assert!(
        connected.get(),
        "the advertised TCP socket must accept a connection"
    );
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_err());
    assert_eq!(fs::read(root.path().join(CONFIG)).unwrap(), manifest);
    assert_eq!(
        Pairing::configured(root.path()).unwrap().unwrap().data,
        previous.data
    );
    assert_eq!(
        fs::read_dir(root.path().join(GENERATIONS)).unwrap().count(),
        1
    );
    println!(
        "Real Soloist advertised the device, accepted a local TCP connection, and stopped on cancellation."
    );
}
