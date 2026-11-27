use std::{os::windows::ffi::OsStrExt, path::Path, ptr};

use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    Security::{
        Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW},
        WinTrust::{
            WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
            WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_STATEACTION_CLOSE,
            WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvCertFromChain,
            WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
        },
    },
};

pub(super) fn verify_spotify(path: &Path, offline: bool) -> Result<(), String> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        ..WINTRUST_FILE_INFO::default()
    };
    let mut state = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: if offline {
            WTD_CACHE_ONLY_URL_RETRIEVAL
        } else {
            0
        },
        ..WINTRUST_DATA::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let hwnd = INVALID_HANDLE_VALUE.cast();
    // Path, union member and state remain allocated through the verify and
    // close calls. Authenticode returns zero only for trusted code signatures.
    let status = unsafe { WinVerifyTrust(hwnd, &mut action, ptr::from_mut(&mut state).cast()) };
    let publisher = (status == 0)
        .then(|| read_publisher(state.hWVTStateData))
        .flatten();
    state.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(hwnd, &mut action, ptr::from_mut(&mut state).cast()) };
    if status != 0 {
        return Err(format!(
            "Windows не подтвердила подпись Spotify Soloist (0x{:08X}).",
            status as u32
        ));
    }
    if publisher.as_deref() != Some("Spotify AB") {
        return Err("Издатель исполняемого файла Spotify Soloist не подтверждён.".into());
    }
    Ok(())
}

fn read_publisher(handle: HANDLE) -> Option<String> {
    // The WinTrust state remains open in our caller and owns the chain.
    // Read only the authenticated primary signer's leaf certificate.
    unsafe {
        let data = WTHelperProvDataFromStateData(handle);
        if data.is_null() {
            return None;
        }
        let signer = WTHelperGetProvSignerFromChain(data, 0, 0, 0);
        if signer.is_null() {
            return None;
        }
        let cert = WTHelperGetProvCertFromChain(signer, 0).as_ref()?.pCert;
        let mut name = [0; 256];
        let count = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            ptr::null(),
            name.as_mut_ptr(),
            name.len() as u32,
        ) as usize;
        if count <= 1 || count > name.len() {
            return None;
        }
        String::from_utf16(&name[..count - 1]).ok()
    }
}
