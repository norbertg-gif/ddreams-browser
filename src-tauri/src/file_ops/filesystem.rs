use std::fs::{self, File, FileTimes, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub trait FileSystemOps: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn is_symlink(&self, path: &Path) -> bool;
    fn metadata(&self, path: &Path) -> io::Result<fs::Metadata>;
    fn same_volume(&self, source: &Path, target: &Path) -> bool;
    fn move_no_replace(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn copy_no_replace(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    fn set_modified(&self, path: &Path, time: SystemTime) -> io::Result<()>;
    /// User-initiated delete: Recycle Bin only, refused where there is no Recycle Bin (USB/network).
    fn recycle_delete(&self, paths: &[PathBuf]) -> Result<(), String>;
    /// Whether deleting from this path can go to the Recycle Bin (fixed local disk).
    fn has_recycle_bin(&self, path: &Path) -> bool;
    /// Rolls back a recycle operation when the catalogue write fails.
    fn restore_recycled(&self, original: &Path) -> Result<(), String>;
}

pub struct NativeFileSystem;

impl FileSystemOps for NativeFileSystem {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
    fn is_symlink(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }
    fn metadata(&self, path: &Path) -> io::Result<fs::Metadata> {
        fs::metadata(path)
    }
    fn same_volume(&self, a: &Path, b: &Path) -> bool {
        volume_key(a) == volume_key(b)
    }
    fn move_no_replace(&self, source: &Path, target: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::{Win32::Storage::FileSystem::MoveFileW, core::PCWSTR};
            let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                MoveFileW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr())).map_err(|e| io::Error::other(e.to_string()))?;
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            if target.exists() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "target exists"));
            }
            fs::rename(source, target)
        }
    }
    fn copy_no_replace(&self, source: &Path, target: &Path) -> io::Result<()> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(target)?;
        let result = (|| {
            io::copy(&mut input, &mut output)?;
            output.set_times(FileTimes::new().set_modified(input.metadata()?.modified()?))?;
            output.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(target);
        }
        result
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }
    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        fs::read_dir(path)?.map(|entry| entry.map(|e| e.path())).collect()
    }
    fn set_modified(&self, path: &Path, time: SystemTime) -> io::Result<()> {
        File::options()
            .write(true)
            .open(path)?
            .set_times(FileTimes::new().set_modified(time))
    }
    fn recycle_delete(&self, paths: &[PathBuf]) -> Result<(), String> {
        for path in paths {
            if !has_recycle_bin(path) {
                return Err(format!(
                    "Na disku so súborom {} nie je Kôš (USB alebo sieťový disk). DDreams tu súbory nemaže, zmazanie by bolo trvalé.",
                    path.display()
                ));
            }
        }
        recycle_windows(paths)?;
        if let Some(left) = paths.iter().find(|p| p.exists()) {
            return Err(format!("Súbor sa nepodarilo presunúť do Koša: {}", left.display()));
        }
        Ok(())
    }
    fn has_recycle_bin(&self, path: &Path) -> bool {
        has_recycle_bin(path)
    }
    fn restore_recycled(&self, original: &Path) -> Result<(), String> {
        let (data, info) = find_recycled(original).ok_or_else(|| {
            format!(
                "Súbor nie je v Koši (Kôš bol možno vyprázdnený): {}",
                original.display()
            )
        })?;
        if original.exists() {
            return Err(format!("Na pôvodnom mieste už je súbor: {}", original.display()));
        }
        if let Some(parent) = original.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        self.move_no_replace(&data, original)
            .map_err(|e| format!("Súbor sa nepodarilo vrátiť z Koša: {} ({e})", original.display()))?;
        let _ = fs::remove_file(info);
        Ok(())
    }
}

/// Fixed local disks have a Recycle Bin; removable and network drives delete permanently.
fn has_recycle_bin(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{Win32::Storage::FileSystem::GetDriveTypeW, core::PCWSTR};
        let Some(root) = path.ancestors().last().map(|r| r.to_path_buf()) else {
            return false;
        };
        let root_text = root.to_string_lossy().to_string();
        if root_text.len() < 2 || !root_text.as_bytes()[1].eq(&b':') {
            return false;
        }
        let wide: Vec<u16> = std::ffi::OsStr::new(&format!("{}\\", &root_text[..2]))
            .encode_wide()
            .chain(Some(0))
            .collect();
        const DRIVE_FIXED: u32 = 3;
        unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) == DRIVE_FIXED }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Windows keeps each recycled file as `$R<id>.<ext>` (content) next to `$I<id>.<ext>` (metadata:
/// version, size, deletion time, original path) in `<drive>\$Recycle.Bin\<user SID>\`.
/// Returns (`$R` data file, `$I` info file) of the most recently deleted match.
fn find_recycled(original: &Path) -> Option<(PathBuf, PathBuf)> {
    let text = original.to_string_lossy().to_string();
    if text.len() < 2 {
        return None;
    }
    let bin = PathBuf::from(format!("{}\\$Recycle.Bin", &text[..2]));
    let wanted = text.replace('/', "\\").to_lowercase();
    let mut best: Option<(u64, PathBuf, PathBuf)> = None;
    for sid in fs::read_dir(&bin).ok()?.flatten() {
        let Ok(entries) = fs::read_dir(sid.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("$I") {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()) else { continue };
            let Some((deleted, path)) = parse_recycle_info(&bytes) else {
                continue;
            };
            if path.to_lowercase() != wanted {
                continue;
            }
            let data = entry.path().with_file_name(format!("$R{}", &name[2..]));
            if !data.exists() {
                continue;
            }
            if best.as_ref().map(|b| deleted > b.0).unwrap_or(true) {
                best = Some((deleted, data, entry.path()));
            }
        }
    }
    best.map(|(_, data, info)| (data, info))
}

/// Parses a `$I` file: v1 = 8 version, 8 size, 8 FILETIME, 520 bytes path; v2 = same + 4-byte length + path.
pub(super) fn parse_recycle_info(bytes: &[u8]) -> Option<(u64, String)> {
    if bytes.len() < 28 {
        return None;
    }
    let version = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let deleted = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let utf16 = |raw: &[u8]| -> String {
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|u| *u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    };
    match version {
        1 => Some((deleted, utf16(bytes.get(24..(24 + 520).min(bytes.len()))?))),
        2 => {
            let len = u32::from_le_bytes(bytes[24..28].try_into().ok()?) as usize;
            Some((deleted, utf16(bytes.get(28..(28 + len * 2).min(bytes.len()))?)))
        }
        _ => None,
    }
}

pub(super) fn volume_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        use std::path::Component;
        for part in path.components() {
            if let Component::Prefix(prefix) = part {
                return prefix.as_os_str().to_string_lossy().to_lowercase();
            }
        }
    }
    path.components()
        .next()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[cfg(windows)]
fn recycle_windows(paths: &[PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::HWND,
            UI::Shell::{
                FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
                SHFileOperationW,
            },
        },
        core::PCWSTR,
    };
    if paths.is_empty() {
        return Ok(());
    }
    let mut from = Vec::<u16>::new();
    for path in paths {
        from.extend(path.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: HWND(std::ptr::null_mut()),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        pTo: PCWSTR::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut operation) };
    if code != 0 || operation.fAnyOperationsAborted.as_bool() {
        Err(format!("Nepodarilo sa presunúť kópiu do Koša (kód {code})"))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn recycle_windows(_paths: &[PathBuf]) -> Result<(), String> {
    Err("Kôš je dostupný iba vo Windows".into())
}

#[cfg(windows)]
pub(crate) fn explorer_select_arg(path: &Path) -> String {
    let escaped = path.to_string_lossy().replace('"', "\"\"");
    format!("/select,\"{escaped}\"")
}
