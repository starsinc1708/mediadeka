use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use super::KEY_BYTES;

const ENCRYPTED_BYTES: u64 = 64 * 1024;
const SAVE_ERROR: &str = "Windows не смогла защитить ключ Spotify Soloist.";
const READ_ERROR: &str = "Windows не смогла прочитать ключ Spotify Soloist.";

pub(super) fn save(path: &Path, key: &str) -> Result<(), String> {
    let encrypted = platform::protect(key.as_bytes()).map_err(|_| SAVE_ERROR)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| SAVE_ERROR)?;
    file.write_all(&encrypted)
        .and_then(|()| file.sync_all())
        .map_err(|_| SAVE_ERROR.into())
}

pub(super) fn load(path: &Path) -> Result<String, String> {
    let mut encrypted = Vec::new();
    File::open(path)
        .map_err(|_| READ_ERROR)?
        .take(ENCRYPTED_BYTES + 1)
        .read_to_end(&mut encrypted)
        .map_err(|_| READ_ERROR)?;
    if encrypted.is_empty() || encrypted.len() as u64 > ENCRYPTED_BYTES {
        return Err(READ_ERROR.into());
    }
    let decrypted = platform::unprotect(&encrypted).map_err(|_| READ_ERROR)?;
    if decrypted.is_empty() || decrypted.len() > KEY_BYTES {
        return Err(READ_ERROR.into());
    }
    String::from_utf8(decrypted).map_err(|_| READ_ERROR.into())
}

#[cfg(windows)]
mod platform {
    use std::{io, ptr, slice};

    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    pub(super) fn protect(input: &[u8]) -> io::Result<Vec<u8>> {
        transform(input, false)
    }

    pub(super) fn unprotect(input: &[u8]) -> io::Result<Vec<u8>> {
        transform(input, true)
    }

    fn transform(input: &[u8], decrypt: bool) -> io::Result<Vec<u8>> {
        let source = CRYPT_INTEGER_BLOB {
            cbData: input
                .len()
                .try_into()
                .map_err(|_| io::ErrorKind::InvalidInput)?,
            pbData: input.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: Both blobs stay live through the synchronous call. DPAPI reads the input,
        // allocates the output with LocalAlloc and uses the current Windows user's protection.
        let success = unsafe {
            if decrypt {
                CryptUnprotectData(
                    &source,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptProtectData(
                    &source,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            }
        };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        if output.pbData.is_null() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        // SAFETY: Successful DPAPI returns this allocation, valid for cbData bytes.
        let bytes =
            unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        if decrypt {
            for index in 0..output.cbData as usize {
                // SAFETY: Each index is in the allocation. Volatile clears the decrypted
                // memory even though the allocator releases it immediately afterward.
                unsafe { ptr::write_volatile(output.pbData.add(index), 0) };
            }
        }
        // SAFETY: The output buffer passed from DPAPI is freed exactly once.
        unsafe { LocalFree(output.pbData.cast()) };
        Ok(bytes)
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    pub(super) fn protect(_: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(super) fn unprotect(_: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
