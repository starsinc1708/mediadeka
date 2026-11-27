use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    os::windows::ffi::OsStrExt,
    ptr::{self, NonNull},
};

use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LoadLibraryExW,
    },
};

use super::{AudioDevice, DEFAULT_AUDIO_DEVICE, quality::AudioQuality};

// ABI from the libmpv SDK's mpv/client.h. The handle and all calls stay on the
// playback worker. Returned event data is copied before calling wait_event again.
const FORMAT_NONE: c_int = 0;
const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const MIN_CLIENT_VERSION: u64 = (2 << 16) | 3;

type Handle = *mut c_void;
type RawProc = unsafe extern "system" fn() -> isize;

#[repr(C)]
struct RawEvent {
    id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *const c_void,
}

#[repr(C)]
struct PropertyEvent {
    name: *const c_char,
    format: c_int,
    data: *const c_void,
}

#[repr(C)]
struct EndFileEvent {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}

pub(super) enum Event {
    Start(i64),
    End { id: i64, result: Result<(), String> },
    Loaded,
    AudioChanged,
    Seek,
    Restarted,
    Paused(bool),
    Buffering(bool),
    Position(f64),
    Duration(Option<f64>),
    Seekable(bool),
    Volume(f64),
    Devices,
    Device,
    DecodeError,
    Overflow,
    UnexpectedStop,
    Other,
}

struct Library(HMODULE);

impl Library {
    fn load() -> Result<Self, String> {
        let path = std::env::current_exe()
            .map_err(|_| "Не удалось определить путь к экземпляру Mediadeka.")?
            .with_file_name("libmpv-2.dll");
        let filename: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        // The path is absolute and NUL-terminated. Dependency search excludes
        // the current directory and PATH, even if launched from another folder.
        let handle = unsafe {
            LoadLibraryExW(
                filename.as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        };
        if handle.is_null() {
            return Err("Не удалось загрузить libmpv-2.dll из папки Mediadeka.".into());
        }
        Ok(Self(handle))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // This owns the reference returned by LoadLibraryExW.
        unsafe { FreeLibrary(self.0) };
    }
}

macro_rules! mpv_api {
    ($($name:ident: $ty:ty),* $(,)?) => {
        struct Api {
            $($name: $ty,)*
            _library: Library,
        }

        impl Api {
            fn load() -> Result<Self, String> {
                let library = Library::load()?;
                $(
                    // The SDK declares this symbol with the signature given below.
                    let $name = unsafe {
                        let name = concat!(stringify!($name), "\0");
                        let address = GetProcAddress(library.0, name.as_ptr())
                            .ok_or_else(|| format!("В libmpv отсутствует {}.", stringify!($name)))?;
                        std::mem::transmute::<RawProc, $ty>(address)
                    };
                )*
                Ok(Self { $($name,)* _library: library })
            }
        }
    };
}

mpv_api! {
    mpv_client_api_version: unsafe extern "C" fn() -> std::ffi::c_ulong,
    mpv_create: unsafe extern "C" fn() -> Handle,
    mpv_initialize: unsafe extern "C" fn(Handle) -> c_int,
    mpv_terminate_destroy: unsafe extern "C" fn(Handle),
    mpv_error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    mpv_set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    mpv_command: unsafe extern "C" fn(Handle, *const *const c_char) -> c_int,
    mpv_get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    mpv_get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    mpv_set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    mpv_observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    mpv_request_log_messages: unsafe extern "C" fn(Handle, *const c_char) -> c_int,
    mpv_wait_event: unsafe extern "C" fn(Handle, f64) -> *const RawEvent,
    mpv_free: unsafe extern "C" fn(*mut c_void),
}

pub(super) struct Mpv {
    api: Api,
    handle: NonNull<c_void>,
}

impl Mpv {
    pub(super) fn open() -> Result<Self, String> {
        let mpv = Self::initialize(&[])?;
        for (name, format) in [
            (c"pause", FORMAT_FLAG),
            (c"paused-for-cache", FORMAT_FLAG),
            (c"time-pos", FORMAT_DOUBLE),
            (c"duration", FORMAT_DOUBLE),
            (c"seekable", FORMAT_FLAG),
            (c"volume", FORMAT_DOUBLE),
            (c"audio-device-list", FORMAT_NONE),
            (c"audio-device", FORMAT_NONE),
        ] {
            // libmpv copies the property name while registering this observation.
            let result =
                unsafe { (mpv.api.mpv_observe_property)(mpv.raw(), 0, name.as_ptr(), format) };
            mpv.check(result)?;
        }
        Ok(mpv)
    }

    pub(super) fn open_pcm_decode(
        output: &str,
        sample_rate_hz: u32,
        max_seconds: u32,
    ) -> Result<Self, String> {
        let output = CString::new(output).map_err(|_| "Недопустимый путь проверки аудио.")?;
        let rate = CString::new(sample_rate_hz.to_string()).expect("numeric sample rate");
        let end = CString::new(max_seconds.to_string()).expect("numeric duration");
        let mpv = Self::initialize(&[
            (c"ao", c"pcm"),
            (c"ao-pcm-file", &output),
            (c"ao-pcm-waveheader", c"no"),
            (c"ao-pcm-append", c"yes"),
            (c"audio-format", c"s16"),
            (c"audio-samplerate", &rate),
            (c"audio-channels", c"stereo"),
            (c"autoload-files", c"no"),
            (c"pause", c"yes"),
            (c"end", &end),
            (c"demuxer", c"lavf"),
            // Only standalone audio containers are accepted into the profile cache.
            // A remote playlist must not open other resources during validation.
            (
                c"demuxer-lavf-o",
                c"format_whitelist=[aac,aiff,ape,asf,caf,flac,matroska,mov,mp3,ogg,tta,wav,wv],protocol_whitelist=file",
            ),
        ])?;
        // Request only errors. The log text can contain paths and is not retained.
        mpv.check(unsafe { (mpv.api.mpv_request_log_messages)(mpv.raw(), c"error".as_ptr()) })?;
        Ok(mpv)
    }

    fn initialize(options: &[(&CStr, &CStr)]) -> Result<Self, String> {
        let api = Api::load()?;
        // Both symbols are available before creating a handle. libmpv major
        // versions are ABI incompatible, and playlist entry IDs require 2.3.
        let version = u64::from(unsafe { (api.mpv_client_api_version)() });
        if version >> 16 != MIN_CLIENT_VERSION >> 16 || version < MIN_CLIENT_VERSION {
            return Err("Версия libmpv несовместима с Mediadeka.".into());
        }
        let handle = NonNull::new(unsafe { (api.mpv_create)() })
            .ok_or("Не удалось создать аудиодвижок libmpv.")?;
        let mpv = Self { api, handle };
        let defaults = [
            (c"config", c"no"),
            (c"load-scripts", c"no"),
            (c"ytdl", c"no"),
            (c"vid", c"no"),
            (c"idle", c"yes"),
            (c"network-timeout", c"15"),
            (c"audio-fallback-to-null", c"no"),
        ];
        for (name, value) in defaults.into_iter().chain(options.iter().copied()) {
            // The handle is created but not initialized, and arguments live for the call.
            let result = unsafe {
                (mpv.api.mpv_set_option_string)(mpv.raw(), name.as_ptr(), value.as_ptr())
            };
            mpv.check(result)?;
        }
        // The handle remains live until Drop, including when initialize fails.
        mpv.check(unsafe { (mpv.api.mpv_initialize)(mpv.raw()) })?;
        Ok(mpv)
    }

    pub(super) fn load(&self, input: &str, position: f64) -> Result<i64, String> {
        self.command(&["stop"])?;
        self.set(c"pause", "no")?;
        self.enqueue(input, position)
    }

    pub(super) fn enqueue(&self, input: &str, position: f64) -> Result<i64, String> {
        // Appending to an empty, idle mpv playlist gives us the entry ID before
        // a fast decoder error or a later replacement can change the entry.
        if position > 0. {
            self.command(&[
                "loadfile",
                input,
                "append",
                "-1",
                &format!("start={position:.3}"),
            ])?;
        } else {
            self.command(&["loadfile", input, "append"])?;
        }
        let id = self.integer(c"playlist/0/id")?;
        self.command(&["playlist-play-index", "0"])?;
        Ok(id)
    }

    pub(super) fn command(&self, argv: &[&str]) -> Result<(), String> {
        let strings: Vec<_> = argv
            .iter()
            .map(|arg| CString::new(*arg))
            .collect::<Result<_, _>>()
            .map_err(|_| "В параметре libmpv есть недопустимый символ.")?;
        let arguments: Vec<_> = strings
            .iter()
            .map(|arg| arg.as_ptr())
            .chain([ptr::null()])
            .collect();
        // libmpv reads the NUL-terminated vector and strings during this call.
        self.check(unsafe { (self.api.mpv_command)(self.raw(), arguments.as_ptr()) })
    }

    pub(super) fn set(&self, name: &CStr, value: &str) -> Result<(), String> {
        let value = CString::new(value).map_err(|_| "Значение параметра libmpv недопустимо.")?;
        // Both strings stay valid until libmpv finishes the operation.
        self.check(unsafe {
            (self.api.mpv_set_property_string)(self.raw(), name.as_ptr(), value.as_ptr())
        })
    }

    fn integer(&self, name: &CStr) -> Result<i64, String> {
        let mut output = 0_i64;
        // FORMAT_INT64 requires writable i64 storage for the duration of the call.
        let result = unsafe {
            (self.api.mpv_get_property)(
                self.raw(),
                name.as_ptr(),
                FORMAT_INT64,
                ptr::from_mut(&mut output).cast(),
            )
        };
        self.check(result).map(|()| output)
    }

    pub(super) fn flag(&self, name: &CStr) -> Option<bool> {
        let mut output = 0_i32;
        // FORMAT_FLAG requires writable C int storage for the duration of the call.
        let result = unsafe {
            (self.api.mpv_get_property)(
                self.raw(),
                name.as_ptr(),
                FORMAT_FLAG,
                ptr::from_mut(&mut output).cast(),
            )
        };
        (result >= 0).then_some(output != 0)
    }

    pub(super) fn double(&self, name: &CStr) -> Option<f64> {
        let mut output = 0_f64;
        // FORMAT_DOUBLE requires writable f64 storage for the duration of the call.
        let result = unsafe {
            (self.api.mpv_get_property)(
                self.raw(),
                name.as_ptr(),
                FORMAT_DOUBLE,
                ptr::from_mut(&mut output).cast(),
            )
        };
        (result >= 0 && output.is_finite()).then_some(output)
    }

    pub(super) fn string(&self, name: &CStr) -> Option<String> {
        // A non-null result is a string allocated by libmpv, freed after copying.
        unsafe {
            let result = NonNull::new((self.api.mpv_get_property_string)(
                self.raw(),
                name.as_ptr(),
            ))?;
            let value = CStr::from_ptr(result.as_ptr())
                .to_string_lossy()
                .into_owned();
            (self.api.mpv_free)(result.as_ptr().cast());
            Some(value)
        }
    }

    pub(super) fn audio_quality(&self) -> Option<AudioQuality> {
        let codec = self
            .string(c"current-tracks/audio/codec")
            .or_else(|| self.string(c"audio-codec-name"))?;
        let sample_rate_hz = positive_u32(self.double(c"audio-params/samplerate")?)?;
        let bitrate_bps = self
            .double(c"audio-bitrate")
            .and_then(positive_u32)
            .or_else(|| {
                self.double(c"current-tracks/audio/demux-bitrate")
                    .and_then(positive_u32)
            });
        Some(AudioQuality::new(codec, bitrate_bps, sample_rate_hz))
    }

    pub(super) fn devices(&self) -> Result<Vec<AudioDevice>, String> {
        let count = self.integer(c"audio-device-list/count")?;
        let mut devices = Vec::new();
        for index in 0..count {
            let name = format!("audio-device-list/{index}");
            let name_path =
                CString::new(format!("{name}/name")).map_err(|error| error.to_string())?;
            let Some(id) = self.string(&name_path) else {
                continue;
            };
            if id == DEFAULT_AUDIO_DEVICE {
                devices.push(AudioDevice {
                    id,
                    label: "Системное устройство".into(),
                });
            } else if id.starts_with("wasapi/") {
                let label_path = CString::new(format!("{name}/description"))
                    .map_err(|error| error.to_string())?;
                let label = self.string(&label_path).unwrap_or_else(|| id.clone());
                devices.push(AudioDevice { id, label });
            }
        }
        Ok(devices)
    }

    pub(super) fn next_event(&self) -> Option<Event> {
        self.wait_event(0.)
    }

    pub(super) fn wait_event(&self, seconds: f64) -> Option<Event> {
        // The next wait invalidates the event and its payload. This method
        // copies payload fields or text without retaining any borrowed pointers.
        let event = unsafe { (self.api.mpv_wait_event)(self.raw(), seconds).as_ref()? };
        Some(match event.id {
            0 => return None, // MPV_EVENT_NONE
            1 => Event::UnexpectedStop,
            2 => Event::DecodeError, // Only the offline verifier subscribes to MPV_EVENT_LOG_MESSAGE.
            6 if !event.data.is_null() => Event::Start(unsafe { *event.data.cast::<i64>() }),
            7 if !event.data.is_null() => {
                // MPV_EVENT_END_FILE points to mpv_event_end_file.
                let end = unsafe { &*event.data.cast::<EndFileEvent>() };
                let result = match end.reason {
                    0 => Ok(()),                        // EOF, as opposed to a stop from another file
                    2 | 3 => return Some(Event::Other), // STOP or QUIT
                    4 => Err(self.error(end.error)),
                    _ => Err("Источник вернул список или неподдерживаемый поток.".into()),
                };
                Event::End {
                    id: end.playlist_entry_id,
                    result,
                }
            }
            8 => Event::Loaded,
            18 => Event::AudioChanged,
            20 => Event::Seek,
            21 => Event::Restarted,
            22 if !event.data.is_null() => {
                // MPV_EVENT_PROPERTY_CHANGE points to mpv_event_property.
                let change = unsafe { &*event.data.cast::<PropertyEvent>() };
                property(change)
            }
            24 => Event::Overflow,
            _ => Event::Other,
        })
    }

    fn raw(&self) -> Handle {
        self.handle.as_ptr()
    }

    fn check(&self, code: c_int) -> Result<(), String> {
        if code < 0 {
            Err(self.error(code))
        } else {
            Ok(())
        }
    }

    fn error(&self, code: c_int) -> String {
        // This is a static error description without the potentially sensitive media URL.
        let text = unsafe { CStr::from_ptr((self.api.mpv_error_string)(code)) }.to_string_lossy();
        format!("libmpv: {text} ({code})")
    }
}

fn positive_u32(value: f64) -> Option<u32> {
    (1.0..=f64::from(u32::MAX))
        .contains(&value)
        .then(|| value.round() as u32)
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // Destroy the uniquely owned handle and its workers before unloading the DLL.
        unsafe { (self.api.mpv_terminate_destroy)(self.raw()) };
    }
}

fn property(change: &PropertyEvent) -> Event {
    // An observation with MPV_FORMAT_NONE or an unavailable property has no data.
    let flag = || {
        change.format == FORMAT_FLAG
            && !change.data.is_null()
            && unsafe { *change.data.cast::<c_int>() != 0 }
    };
    let double = || {
        (change.format == FORMAT_DOUBLE && !change.data.is_null())
            .then(|| unsafe { *change.data.cast::<f64>() })
            .filter(|value| value.is_finite())
    };
    // libmpv guarantees a NUL-terminated name for property observations.
    match unsafe { CStr::from_ptr(change.name) }.to_bytes() {
        b"pause" => Event::Paused(flag()),
        b"paused-for-cache" => Event::Buffering(flag()),
        b"time-pos" => double().map_or(Event::Other, Event::Position),
        b"duration" => Event::Duration(double()),
        b"seekable" => Event::Seekable(flag()),
        b"volume" => double().map_or(Event::Other, Event::Volume),
        b"audio-device-list" => Event::Devices,
        b"audio-device" => Event::Device,
        _ => Event::Other,
    }
}
