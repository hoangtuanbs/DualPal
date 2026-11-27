//! Reading RAR archives (RAR 1.5 to 5) through the official UnRAR library.
//!
//! UnRAR is used under its own license: free to use and ship, but not to
//! build a RAR-compatible archiver or re-create RAR compression. Dual Pal
//! only lists and reads RAR archives.
//!
//! The library's API is a cursor over the archive's headers: each file is
//! either skipped or "tested", and a test hands the unpacked bytes to a
//! callback in chunks. That lets a file be streamed out without writing it
//! to disk, and stopped at any chunk.

use std::ffi::c_int;
use std::path::Path;

use unrar_sys as native;

/// One entry of a RAR archive, in archive order.
#[derive(Debug, Clone)]
pub struct RarEntry {
    /// The name as stored, with `/` or `\` between folders.
    pub name: String,
    pub dir: bool,
    pub encrypted: bool,
    /// Unpacked and packed sizes.
    pub size: u64,
    pub packed: u64,
    pub modified_ms: Option<i64>,
    /// Part of a multi-volume set (continued from or into another file).
    pub split: bool,
}

#[derive(Debug)]
pub enum RarError {
    /// The headers are encrypted, or a password is asked for.
    Password,
    /// The callback stopped the read (the reader went away, or it failed).
    Stopped,
    /// UnRAR's own error code.
    Code(c_int),
}

impl RarError {
    /// The next volume of a multi-volume set was needed and not found.
    pub fn is_missing_volume(&self) -> bool {
        matches!(self, RarError::Code(native::ERAR_EOPEN))
    }
}

impl std::fmt::Display for RarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RarError::Password => f.write_str("the archive needs a password"),
            RarError::Stopped => f.write_str("reading was stopped"),
            RarError::Code(c) => f.write_str(match *c {
                native::ERAR_NO_MEMORY => "not enough memory",
                native::ERAR_BAD_DATA => "the data is damaged (checksum error)",
                native::ERAR_BAD_ARCHIVE => "not a valid RAR archive",
                native::ERAR_UNKNOWN_FORMAT => "unknown RAR format",
                native::ERAR_EOPEN => "a volume of the archive could not be opened",
                native::ERAR_EREAD => "read error",
                native::ERAR_MISSING_PASSWORD | native::ERAR_BAD_PASSWORD => {
                    "the archive needs a password"
                }
                _ => "unknown UnRAR error",
            }),
        }
    }
}

type Wide = Vec<native::WCHAR>;

// UnRAR's dll.hpp declares its structs under `#pragma pack(1)`, but the
// `unrar_sys` structs are `repr(C)`, which pads before pointers and shifts
// every later field (the times, the wide comment buffer). These match the
// vendored dll.hpp byte for byte and are passed where UnRAR takes a pointer.

#[repr(C, packed)]
struct OpenData {
    arc_name: *const std::ffi::c_char,
    arc_name_w: *const native::WCHAR,
    open_mode: u32,
    open_result: u32,
    cmt_buf: *mut std::ffi::c_char,
    cmt_buf_size: u32,
    cmt_size: u32,
    cmt_state: u32,
    flags: u32,
    callback: Option<native::Callback>,
    user_data: native::LPARAM,
    op_flags: u32,
    cmt_buf_w: *mut native::WCHAR,
    reserved: [u32; 25],
}

#[repr(C, packed)]
struct Header {
    arc_name: [std::ffi::c_char; 1024],
    arc_name_w: [native::WCHAR; 1024],
    file_name: [std::ffi::c_char; 1024],
    file_name_w: [native::WCHAR; 1024],
    flags: u32,
    pack_size: u32,
    pack_size_high: u32,
    unp_size: u32,
    unp_size_high: u32,
    host_os: u32,
    file_crc: u32,
    file_time: u32,
    unp_ver: u32,
    method: u32,
    file_attr: u32,
    cmt_buf: *mut std::ffi::c_char,
    cmt_buf_size: u32,
    cmt_size: u32,
    cmt_state: u32,
    dict_size: u32,
    hash_type: u32,
    hash: [std::ffi::c_char; 32],
    redir_type: u32,
    redir_name: *mut native::WCHAR,
    redir_name_size: u32,
    dir_target: u32,
    mtime_low: u32,
    mtime_high: u32,
    ctime_low: u32,
    ctime_high: u32,
    atime_low: u32,
    atime_high: u32,
    arc_name_ex: *mut native::WCHAR,
    arc_name_ex_size: u32,
    file_name_ex: *mut native::WCHAR,
    file_name_ex_size: u32,
    reserved: [u32; 982],
}

impl Header {
    fn zeroed() -> Box<Self> {
        // SAFETY: all-zero bytes are a valid Header (numbers, null pointers).
        unsafe { Box::new(std::mem::zeroed()) }
    }
}

/// `path` as a NUL-terminated wide string, as UnRAR's `W` functions take.
fn wide(path: &Path) -> Wide {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy()
            .chars()
            .map(|c| c as native::WCHAR)
            .chain(std::iter::once(0))
            .collect()
    }
}

/// A NUL-terminated wide string from UnRAR, as text.
fn from_wide(w: &[native::WCHAR]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    #[cfg(windows)]
    {
        String::from_utf16_lossy(&w[..end])
    }
    #[cfg(not(windows))]
    {
        w[..end]
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap_or('\u{FFFD}'))
            .collect()
    }
}

/// Takes each chunk of unpacked data; returns false to stop.
type DataFn = dyn FnMut(&[u8]) -> bool;

/// The callback state: where data goes, and whether a password was asked.
struct Sink {
    /// Set only for the length of one [`Archive::process`] call.
    data: Option<&'static mut DataFn>,
    password_asked: bool,
}

extern "C" fn callback(
    msg: native::UINT,
    user: native::LPARAM,
    p1: native::LPARAM,
    p2: native::LPARAM,
) -> c_int {
    // SAFETY: `user` is the `Sink` that `Archive::open` registered, which
    // outlives the handle; UnRAR calls back on the calling thread only.
    let sink = unsafe { &mut *(user as *mut Sink) };
    match msg {
        native::UCM_PROCESSDATA => {
            let Some(data) = sink.data.as_mut() else {
                return 1;
            };
            if p2 <= 0 {
                return 1;
            }
            // SAFETY: UnRAR passes a buffer of `p2` bytes at `p1`.
            let bytes = unsafe { std::slice::from_raw_parts(p1 as *const u8, p2 as usize) };
            if data(bytes) {
                1
            } else {
                -1
            }
        }
        native::UCM_NEEDPASSWORD | native::UCM_NEEDPASSWORDW => {
            sink.password_asked = true;
            -1
        }
        // The next volume of a multi-volume set: go on when it is there
        // (notify), stop when UnRAR would have to ask for it.
        native::UCM_CHANGEVOLUME | native::UCM_CHANGEVOLUMEW => {
            if p2 == native::RAR_VOL_NOTIFY {
                1
            } else {
                -1
            }
        }
        _ => 0,
    }
}

/// An open archive handle, closed when dropped.
struct Archive {
    handle: *const native::Handle,
    flags: u32,
    sink: Box<Sink>,
}

impl Archive {
    fn open(path: &Path, mode: u32) -> Result<Self, RarError> {
        let mut sink = Box::new(Sink {
            data: None,
            password_asked: false,
        });
        // UnRAR on Linux mishandles wide names, so it gets the bytes of the
        // path there (as the `unrar` crate does); elsewhere the wide name.
        #[cfg(unix)]
        let narrow = {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_err(|_| RarError::Code(native::ERAR_EOPEN))?
        };
        #[cfg(not(unix))]
        let narrow = std::ffi::CString::default();
        let name = wide(path);
        let linux = cfg!(any(target_os = "linux", target_os = "netbsd"));
        let mut data = OpenData {
            arc_name: if linux {
                narrow.as_ptr()
            } else {
                std::ptr::null()
            },
            arc_name_w: if linux {
                std::ptr::null()
            } else {
                name.as_ptr()
            },
            open_mode: mode,
            open_result: 0,
            cmt_buf: std::ptr::null_mut(),
            cmt_buf_size: 0,
            cmt_size: 0,
            cmt_state: 0,
            flags: 0,
            callback: Some(callback),
            user_data: &mut *sink as *mut Sink as native::LPARAM,
            op_flags: 0,
            cmt_buf_w: std::ptr::null_mut(),
            reserved: [0; 25],
        };
        // SAFETY: `data` and the names it points to are valid for the call;
        // UnRAR copies the names.
        let handle = unsafe {
            native::RAROpenArchiveEx(&mut data as *mut OpenData as *const native::OpenArchiveDataEx)
        };
        let (open_result, flags) = (data.open_result, data.flags);
        if handle.is_null() || open_result != 0 {
            if !handle.is_null() {
                // SAFETY: a non-null handle from RAROpenArchiveEx.
                unsafe { native::RARCloseArchive(handle) };
            }
            return Err(match open_result as c_int {
                native::ERAR_MISSING_PASSWORD | native::ERAR_BAD_PASSWORD => RarError::Password,
                c => RarError::Code(c),
            });
        }
        let archive = Self {
            handle,
            flags,
            sink,
        };
        if archive.flags & native::ROADF_ENCHEADERS != 0 {
            return Err(RarError::Password);
        }
        Ok(archive)
    }

    /// The next header, or `None` at the end.
    fn next(&mut self) -> Result<Option<Box<Header>>, RarError> {
        let mut h = Header::zeroed();
        // SAFETY: an open handle, and a header laid out as UnRAR's.
        let code = unsafe {
            native::RARReadHeaderEx(
                self.handle,
                &mut *h as *mut Header as *const native::HeaderDataEx,
            )
        };
        match code {
            native::ERAR_SUCCESS => Ok(Some(h)),
            native::ERAR_END_ARCHIVE => Ok(None),
            _ if self.sink.password_asked => Err(RarError::Password),
            c => Err(RarError::Code(c)),
        }
    }

    /// Skip the file after the last header, or test it (unpack it to the
    /// callback) when `test`.
    fn process(&mut self, test: bool) -> Result<(), RarError> {
        let op = if test {
            native::RAR_TEST
        } else {
            native::RAR_SKIP
        };
        // SAFETY: an open handle positioned after a header.
        let code =
            unsafe { native::RARProcessFileW(self.handle, op, std::ptr::null(), std::ptr::null()) };
        if code == native::ERAR_SUCCESS {
            Ok(())
        } else if self.sink.password_asked {
            Err(RarError::Password)
        } else if code == native::ERAR_UNKNOWN || code == native::ERAR_EWRITE {
            // A callback returning -1 shows up as one of these.
            Err(RarError::Stopped)
        } else {
            Err(RarError::Code(code))
        }
    }
}

impl Drop for Archive {
    fn drop(&mut self) {
        // SAFETY: the handle came from RAROpenArchiveEx and is closed once.
        unsafe { native::RARCloseArchive(self.handle) };
    }
}

/// Windows FILETIME (100 ns ticks since 1601) to Unix milliseconds.
fn filetime_ms(low: u32, high: u32) -> Option<i64> {
    let ticks = ((high as u64) << 32) | low as u64;
    (ticks != 0).then(|| (ticks / 10_000) as i64 - 11_644_473_600_000)
}

/// MS-DOS date and time (local time, 2 s steps) to Unix milliseconds,
/// read as UTC: only used when an archive has no exact time.
fn dos_time_ms(t: u32) -> Option<i64> {
    if t == 0 {
        return None;
    }
    let (date, time) = (t >> 16, t & 0xFFFF);
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 0xF) as i64;
    let day = (date & 0x1F) as i64;
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    // Days from 1970-01-01 (civil-from-days inverse, proleptic Gregorian).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = ((time >> 11) * 3600 + ((time >> 5) & 0x3F) * 60 + (time & 0x1F) * 2) as i64;
    Some((days * 86_400 + secs) * 1000)
}

/// Every entry of the archive at `path`.
pub fn list(path: &Path) -> Result<Vec<RarEntry>, RarError> {
    let mut a = Archive::open(path, native::RAR_OM_LIST)?;
    let mut out = Vec::new();
    while let Some(h) = a.next()? {
        let (wide_name, narrow_name) = (h.file_name_w, h.file_name);
        let name = from_wide(&wide_name);
        let name = if name.is_empty() {
            let end = narrow_name.iter().position(|&c| c == 0).unwrap_or(0);
            let bytes: Vec<u8> = narrow_name[..end].iter().map(|&c| c as u8).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            name
        };
        out.push(RarEntry {
            name,
            dir: h.flags & native::RHDF_DIRECTORY != 0,
            encrypted: h.flags & native::RHDF_ENCRYPTED != 0,
            size: ((h.unp_size_high as u64) << 32) | h.unp_size as u64,
            packed: ((h.pack_size_high as u64) << 32) | h.pack_size as u64,
            modified_ms: filetime_ms(h.mtime_low, h.mtime_high).or(dos_time_ms(h.file_time)),
            split: h.flags & (native::RHDF_SPLITBEFORE | native::RHDF_SPLITAFTER) != 0,
        });
        a.process(false)?;
    }
    Ok(out)
}

/// Unpack entry number `index` (in [`list`] order) of the archive at
/// `path`, handing its bytes to `data` in chunks; `data` returns false to
/// stop. Earlier entries are skipped (in a solid archive UnRAR still
/// decodes them, as it must).
pub fn read(
    path: &Path,
    index: usize,
    data: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), RarError> {
    let mut a = Archive::open(path, native::RAR_OM_EXTRACT)?;
    let mut i = 0;
    while a.next()?.is_some() {
        if i == index {
            // SAFETY: only the lifetime changes. The sink holds `data` for
            // the one `process` call below and is cleared before returning
            // (`process` cannot unwind: UnRAR is C++ behind a C ABI).
            let data: &'static mut DataFn = unsafe {
                std::mem::transmute::<&mut (dyn FnMut(&[u8]) -> bool + '_), &'static mut DataFn>(
                    data,
                )
            };
            a.sink.data = Some(data);
            let result = a.process(true);
            a.sink.data = None;
            return result;
        }
        a.process(false)?;
        i += 1;
    }
    Err(RarError::Code(native::ERAR_END_ARCHIVE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structs_match_dll_hpp() {
        // Sizes under `#pragma pack(1)` in the vendored dll.hpp.
        let (p, w) = (
            std::mem::size_of::<usize>(),
            std::mem::size_of::<native::WCHAR>(),
        );
        assert_eq!(std::mem::size_of::<OpenData>(), 5 * p + 7 * 4 + 25 * 4 + p);
        assert_eq!(
            std::mem::size_of::<Header>(),
            2048 + 2048 * w + 11 * 4 + p + 5 * 4 + 32 + 4 + p + 8 * 4 + p + 4 + p + 4 + 982 * 4
        );
    }

    #[test]
    fn times() {
        assert_eq!(filetime_ms(0, 0), None);
        // 2020-09-13T12:26:40Z = 1_600_000_000 s.
        let ticks: u64 = (1_600_000_000 + 11_644_473_600) * 10_000_000;
        assert_eq!(
            filetime_ms(ticks as u32, (ticks >> 32) as u32),
            Some(1_600_000_000_000)
        );
        // 2020-09-13 12:26:40 as a DOS time.
        let date = ((2020 - 1980) << 9) | (9 << 5) | 13;
        let time = (12 << 11) | (26 << 5) | (40 / 2);
        assert_eq!(dos_time_ms((date << 16) | time), Some(1_600_000_000_000));
        assert_eq!(dos_time_ms(0), None);
    }
}
