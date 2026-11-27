use std::{
    fs::{File, OpenOptions, TryLockError},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::providers::ProviderId;

use super::{Result, Store};

const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(25);

impl Store {
    pub(crate) fn lock_connection(
        &self,
        provider: ProviderId,
        cancelled: &AtomicBool,
    ) -> Result<Option<File>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(
                self.database
                    .with_file_name(format!("{}-connection.lock", provider.as_str())),
            )?;
        let deadline = Instant::now() + LOCK_WAIT;
        while !cancelled.load(Ordering::Relaxed) {
            match file.try_lock() {
                Ok(()) => return Ok(Some(file)),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(LOCK_POLL)
                }
                Err(TryLockError::WouldBlock) => break,
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        Ok(None)
    }
}
