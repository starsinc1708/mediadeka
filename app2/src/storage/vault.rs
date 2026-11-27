//! Win32 vault calls follow `app/src/credentials.rs` at `0149b55043217b7d3215473c2da0435936507f8a`.

use std::io;

#[cfg(windows)]
mod platform {
    use std::{
        env,
        fs::{self, File, OpenOptions},
        io,
        os::windows::fs::OpenOptionsExt,
        path::PathBuf,
        ptr, slice, thread,
        time::{Duration, Instant},
    };

    use windows_sys::Win32::{
        Foundation::{ERROR_NOT_FOUND, ERROR_SHARING_VIOLATION},
        Security::Credentials::{
            CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
            CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
        },
    };

    pub(super) const BLOB_LIMIT: usize = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;
    const LOCK_TIMEOUT: Duration = Duration::from_secs(3);
    const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

    pub(super) fn guard() -> io::Result<File> {
        // Credential Manager is shared by every app2 profile in this Windows user account.
        let directory = env::var_os("LOCALAPPDATA")
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(path).join("Mediadeka"))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Windows profile path is missing")
            })?;
        fs::create_dir_all(&directory)?;
        let path = directory.join("app2-vault.lock");
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&path)
            {
                Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Windows credential storage is busy",
                        ));
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                result => return result,
            }
        }
    }

    pub(super) fn read(target: &str) -> io::Result<Option<Vec<u8>>> {
        let target = encode(target);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: Both arguments live through the call. Windows owns the output until CredFree.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        // SAFETY: CredReadW initialized the structure. Its blob lives until CredFree.
        let value = unsafe {
            let record = &*credential;
            let length = record.CredentialBlobSize as usize;
            if length == 0 {
                Ok(Vec::new())
            } else if length <= BLOB_LIMIT && !record.CredentialBlob.is_null() {
                Ok(slice::from_raw_parts(record.CredentialBlob, length).to_vec())
            } else {
                Err(super::invalid_blob())
            }
        };
        // SAFETY: `credential` is the buffer returned by CredReadW. The bytes have been copied.
        unsafe { CredFree(credential.cast()) };
        value.map(Some)
    }

    pub(super) fn write(target: &str, value: &[u8]) -> io::Result<()> {
        let mut target = encode(target);
        let record = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: value.len() as u32,
            CredentialBlob: value.as_ptr().cast_mut(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: CredWriteW copies the blob and target synchronously while they are live.
        if unsafe { CredWriteW(&record, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn delete(target: &str) -> io::Result<()> {
        let target = encode(target);
        // SAFETY: The target is a NUL-terminated buffer that lives through the call.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn encode(target: &str) -> Vec<u16> {
        target.encode_utf16().chain([0]).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    pub(super) const BLOB_LIMIT: usize = 2560;

    pub(super) fn guard() -> io::Result<std::fs::File> {
        Err(unsupported())
    }

    pub(super) fn read(_: &str) -> io::Result<Option<Vec<u8>>> {
        Err(unsupported())
    }

    pub(super) fn write(_: &str, _: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    pub(super) fn delete(_: &str) -> io::Result<()> {
        Err(unsupported())
    }

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows Credential Manager is unavailable",
        )
    }
}

const INLINE: u8 = 1;
const CHUNKED: u8 = 2;
const PAYLOAD_LIMIT: usize = 64 * 1024;
use platform::BLOB_LIMIT;

pub(super) fn read(target: &str) -> io::Result<Option<Vec<u8>>> {
    let _guard = platform::guard()?;
    read_locked(target)
}

fn read_locked(target: &str) -> io::Result<Option<Vec<u8>>> {
    let Some(blob) = platform::read(target)? else {
        return Ok(None);
    };
    match blob.split_first() {
        Some((&INLINE, value)) if !value.is_empty() => Ok(Some(value.to_vec())),
        Some((&CHUNKED, size)) => {
            let length = chunked_size(size)?;
            let mut payload = Vec::with_capacity(length);
            for index in 0..length.div_ceil(BLOB_LIMIT) {
                let bytes =
                    platform::read(&chunk_target(target, index))?.ok_or_else(invalid_blob)?;
                if bytes.len() != (length - payload.len()).min(BLOB_LIMIT) {
                    return Err(invalid_blob());
                }
                payload.extend(bytes);
            }
            Ok(Some(payload))
        }
        _ => Err(invalid_blob()),
    }
}

pub(super) fn write(target: &str, value: &[u8]) -> io::Result<()> {
    let length = value.len();
    if length == 0 || length > PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid credential payload size",
        ));
    }
    let _guard = platform::guard()?;
    let mut written = 0;
    // The caller uses a new immutable target for each version. Publish the manifest last.
    let result = (|| {
        if length < BLOB_LIMIT {
            let mut blob = Vec::with_capacity(length + 1);
            blob.push(INLINE);
            blob.extend_from_slice(value);
            platform::write(target, &blob)?;
        } else {
            for (index, bytes) in value.chunks(BLOB_LIMIT).enumerate() {
                platform::write(&chunk_target(target, index), bytes)?;
                written += 1;
            }
            let mut header = [CHUNKED; 5];
            header[1..].copy_from_slice(&(length as u32).to_le_bytes());
            platform::write(target, &header)?;
        }
        // SQLite may publish the reference only after the complete payload comes back from Windows.
        if read_locked(target)?.as_deref() != Some(value) {
            return Err(io::Error::other(
                "Windows credential storage could not verify the new record",
            ));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = platform::delete(target);
        for index in 0..written {
            let _ = platform::delete(&chunk_target(target, index));
        }
    }
    result
}

pub(super) fn delete(target: &str) -> io::Result<()> {
    let _guard = platform::guard()?;
    let Some(blob) = platform::read(target)? else {
        return Ok(());
    };
    if let Some((&CHUNKED, size)) = blob.split_first() {
        for index in 0..chunked_size(size)?.div_ceil(BLOB_LIMIT) {
            platform::delete(&chunk_target(target, index))?;
        }
    }
    platform::delete(target)
}

fn chunk_target(target: &str, index: usize) -> String {
    format!("{target}/{index}")
}

fn chunked_size(size: &[u8]) -> io::Result<usize> {
    let bytes = size.try_into().map_err(|_| invalid_blob())?;
    let length = u32::from_le_bytes(bytes) as usize;
    if !(BLOB_LIMIT..=PAYLOAD_LIMIT).contains(&length) {
        return Err(invalid_blob());
    }
    Ok(length)
}

fn invalid_blob() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid credential payload")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn windows_vault_round_trips_inline_and_chunked_credentials() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for length in [18, BLOB_LIMIT, 7503] {
            let target = format!("Mediadeka/app2/test/{}/{now}/{length}", std::process::id());
            let value: Vec<u8> = (0..length).map(|index| (index % 251) as u8).collect();
            write(&target, &value).unwrap();
            let found = read(&target);
            let deleted = delete(&target);
            let found = found
                .unwrap()
                .expect("Windows credential is missing after write");
            assert!(found == value, "Windows credential changed after write");
            deleted.unwrap();
            assert_eq!(read(&target).unwrap(), None);
            for index in 0..length.div_ceil(BLOB_LIMIT) {
                assert_eq!(platform::read(&chunk_target(&target, index)).unwrap(), None);
            }
        }
    }
}
