//! Windows backend for the book-local receipt store.
//!
//! Mirrors the Unix guarantees in `storage.rs` with native Windows mechanisms:
//!
//! | Unix                                   | Windows (this module)                                                  |
//! |----------------------------------------|------------------------------------------------------------------------|
//! | `openat` relative to a directory fd    | `NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory` = directory handle |
//! | `O_NOFOLLOW`                           | `FILE_OPEN_REPARSE_POINT` + `OBJ_DONT_REPARSE`, then refuse any handle whose attributes carry `FILE_ATTRIBUTE_REPARSE_POINT` |
//! | `O_DIRECTORY` / regular-file check     | `FILE_DIRECTORY_FILE` / `FILE_NON_DIRECTORY_FILE`, plus an attribute check and `GetFileType == FILE_TYPE_DISK` |
//! | `mkdirat(0o700)` + `fchmod(0o700)`     | protected DACL with a single full-control ACE for the current user (inherited by children) |
//! | `O_CREAT|O_EXCL`, mode 0600            | `FILE_CREATE` with the same owner-only protected DACL                  |
//! | `st_uid == getuid()`                   | directory owner SID equals the token user SID (or the token's default owner SID) |
//! | `fsync(file)`                          | `FlushFileBuffers` (via `File::sync_all`)                              |
//! | `linkat` (never replaces)              | `NtSetInformationFile(FileRenameInformation)` relative to the directory handle with `ReplaceIfExists = FALSE` |
//! | `fsync(directory)`                     | `FlushFileBuffers` on the directory handle; failure is an error (fail closed) |
//! | `unlinkat`                             | delete-on-close disposition set on a handle opened relative to the directory |
//!
//! Known differences, all resolved toward refusing:
//! - Placement is a no-replace *rename* of the temporary file rather than a hard link, so there is no window in
//!   which both names exist; the end state is identical to Unix (temp name gone, final name present, never
//!   overwriting an existing entry).
//! - Directory flushing is only supported by some file systems (NTFS, ReFS). Where `FlushFileBuffers` on a
//!   directory handle fails (for example FAT/exFAT removable drives) the write is refused and the new file removed,
//!   matching the Unix behavior when `fsync` on the directory fails.
//! - Owner-only means exactly one ACE for the current user. Unlike root on Unix, `SYSTEM` and Administrators are
//!   not granted access by the DACL; they can still reach the data through backup/restore privileges or by taking
//!   ownership, as with any per-user file.
//! - Path-based opens (the book's parent folder and `verify_staged`) only refuse a reparse point in the final
//!   component, exactly like Unix `O_NOFOLLOW`. Every path component is additionally checked for reserved device
//!   names, alternate data streams and trailing dots or spaces, and device namespace paths (`\\.\`) are refused.
#![allow(unsafe_code)]

use super::io;
use crate::FbResult;
use std::{
    ffi::c_void,
    fs::File,
    mem::{offset_of, size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, Prefix},
    ptr::{null, null_mut},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_CREATE, FILE_DIRECTORY_FILE, FILE_DISPOSITION_INFORMATION, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
            FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT,
            FileDispositionInformation, FileRenameInformation, NtCreateFile, NtSetInformationFile,
        },
    },
    Win32::{
        Foundation::{
            ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, LocalFree, NTSTATUS, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE,
            RtlNtStatusToDosError, UNICODE_STRING,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
            },
            ACL, DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
            OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            SECURITY_DESCRIPTOR, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER, TokenOwner, TokenUser,
        },
        Storage::FileSystem::{
            CreateFileW, DELETE, FILE_ADD_FILE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_NONE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_TRAVERSE, FILE_TYPE_DISK, FileAttributeTagInfo, GetFileInformationByHandleEx, GetFileType,
            OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE, WRITE_DAC,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
};

/// Access requested on the receipts directory: list, create entries, flush (needs `FILE_ADD_FILE`, the directory
/// form of `FILE_WRITE_DATA`), and inspect/replace its DACL.
const DIR_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC | SYNCHRONIZE;
/// Directories are shared fully, like an open directory fd on Unix: every later operation is relative to the
/// handle, so a concurrent rename of the folder cannot redirect a write.
const DIR_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

fn last_error() -> crate::FbError {
    io(std::io::Error::last_os_error())
}
fn nt_error(status: NTSTATUS) -> crate::FbError {
    // SAFETY: pure status-code translation.
    io(std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32))
}
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// A single Windows path component that cannot be redirected by Win32 name parsing: no reserved device names
/// (`CON`, `NUL`, `COM1`, `LPT¹` and so on, with or without an extension), no alternate data stream (`:`), no
/// wildcard or separator characters, no control characters and no trailing dot or space.
pub(super) fn valid_component(name: &str) -> bool {
    const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    if name.is_empty() || name == "." || name == ".." || name.ends_with(['.', ' ']) {
        return false;
    }
    if name.chars().any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return false;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    !((prefix == "COM" || prefix == "LPT")
        && rest.len() == 1
        && (rest[0].is_ascii_digit() || matches!(rest[0], '¹' | '²' | '³')))
}

/// Refuse device-namespace paths and any component that `valid_component` rejects. Drive and UNC prefixes are
/// allowed; so are verbatim (`\\?\`) forms of them, which are what `canonicalize` returns.
fn check_path(path: &Path) -> FbResult<()> {
    for c in path.components() {
        match c {
            Component::Prefix(p) => {
                if !matches!(
                    p.kind(),
                    Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(..) | Prefix::VerbatimUNC(..)
                ) {
                    return Err(io("Unsupported path prefix"));
                }
            }
            Component::Normal(n) => {
                if !n.to_str().is_some_and(valid_component) {
                    return Err(io("Unsupported path component"));
                }
            }
            Component::RootDir | Component::CurDir | Component::ParentDir => {}
        }
    }
    Ok(())
}

fn attributes(h: &impl AsRawHandle) -> FbResult<u32> {
    // SAFETY: plain-old-data out parameter of the declared size.
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
    // SAFETY: the handle is open for the call, and info is a live local of the size passed.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h.as_raw_handle() as HANDLE,
            FileAttributeTagInfo,
            (&raw mut info).cast::<c_void>(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(info.FileAttributes)
}

/// The Windows form of `O_NOFOLLOW` plus `O_DIRECTORY`/regular-file checks: the handle must be on a disk file
/// system, must not be a reparse point (symlink, junction, mount point, cloud placeholder…) and must be of the
/// expected kind.
pub(super) fn refuse_unexpected(h: &impl AsRawHandle, want_directory: bool) -> FbResult<()> {
    // SAFETY: the handle is valid for the duration of the borrow.
    if unsafe { GetFileType(h.as_raw_handle() as HANDLE) } != FILE_TYPE_DISK {
        return Err(io("Not a file on disk"));
    }
    let a = attributes(h)?;
    if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io("Links and reparse points are not allowed"));
    }
    if (a & FILE_ATTRIBUTE_DIRECTORY != 0) != want_directory {
        return Err(io(if want_directory { "Not a folder" } else { "Not a regular file" }));
    }
    Ok(())
}

/// An owner-only protected security descriptor for the current user, allocated by `LocalAlloc`.
struct OwnerOnly(PSECURITY_DESCRIPTOR);
impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.0) };
    }
}
impl OwnerOnly {
    /// `D:P(A;OICI;FA;;;<user>)` for directories, `D:P(A;;FA;;;<user>)` for files: protected (no inherited ACEs),
    /// one full-control ACE for the current user. This is the Windows equivalent of 0700 / 0600.
    fn new(directory: bool) -> FbResult<Self> {
        let token = Token::current()?;
        let user = token.info(TokenUser)?;
        // SAFETY: the buffer was filled by GetTokenInformation(TokenUser) and is suitably aligned.
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text: *mut u16 = null_mut();
        // SAFETY: sid points into `user`, which outlives this call; text is freed below.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(last_error());
        }
        // SAFETY: ConvertSidToStringSidW returns a NUL-terminated LocalAlloc'd string.
        let sid_text = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            s
        };
        let flags = if directory { "OICI" } else { "" };
        let sddl = wide(std::ffi::OsStr::new(&format!("D:P(A;{flags};FA;;;{sid_text})")));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: sddl is NUL-terminated; sd is freed by Drop.
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) }
            == 0
        {
            return Err(last_error());
        }
        Ok(Self(sd))
    }
    fn dacl(&self) -> FbResult<*mut ACL> {
        let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut());
        // SAFETY: self.0 is a valid self-relative security descriptor.
        if unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) } == 0 || present == 0 {
            return Err(io("Could not build the owner-only access list"));
        }
        Ok(dacl)
    }
}

struct Token(OwnedHandle);
impl Token {
    fn current() -> FbResult<Self> {
        let mut h: HANDLE = null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle; h is owned on success.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut h) } == 0 {
            return Err(last_error());
        }
        // SAFETY: h is a freshly opened, owned handle.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(h) }))
    }
    /// Token information in an 8-byte aligned buffer.
    fn info(&self, class: i32) -> FbResult<Vec<u64>> {
        let mut needed = 0u32;
        // SAFETY: size query with a null buffer.
        unsafe { GetTokenInformation(self.0.as_raw_handle() as HANDLE, class, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(last_error());
        }
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: buf holds at least `needed` bytes.
        if unsafe {
            GetTokenInformation(self.0.as_raw_handle() as HANDLE, class, buf.as_mut_ptr().cast(), needed, &mut needed)
        } == 0
        {
            return Err(last_error());
        }
        Ok(buf)
    }
}

/// The Windows equivalent of `st_uid == getuid()`: the directory's owner must be the current user, or the token's
/// default owner for new objects (which differs from the user only in elevated administrator sessions).
fn owned_by_current_user(h: &impl AsRawHandle) -> FbResult<bool> {
    let token = Token::current()?;
    let user = token.info(TokenUser)?;
    let default_owner = token.info(TokenOwner)?;
    let mut owner: PSID = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: owner points into sd, which is freed below after the comparisons.
    let err = unsafe {
        GetSecurityInfo(
            h.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(io(std::io::Error::from_raw_os_error(err as i32)));
    }
    // SAFETY: both token buffers were filled by GetTokenInformation for the matching classes.
    let same = unsafe {
        !owner.is_null()
            && (EqualSid(owner, (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid) != 0
                || EqualSid(owner, (*default_owner.as_ptr().cast::<TOKEN_OWNER>()).Owner) != 0)
    };
    // SAFETY: sd was allocated by GetSecurityInfo.
    unsafe { LocalFree(sd) };
    Ok(same)
}

/// `openat` for Windows: open exactly one component relative to `dir`, never following a reparse point.
/// `OBJ_DONT_REPARSE` needs Windows 10 1709 or later; older systems refuse the open.
fn open_at(
    dir: &impl AsRawHandle,
    name: &str,
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
    security: Option<&OwnerOnly>,
) -> FbResult<OwnedHandle> {
    if !valid_component(name) {
        return Err(io("Bad stored name."));
    }
    let wide: Vec<u16> = name.encode_utf16().collect();
    let bytes = u16::try_from(wide.len() * 2).map_err(|_| io("Name too long"))?;
    let object_name = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: wide.as_ptr().cast_mut() };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: dir.as_raw_handle() as HANDLE,
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security.map_or(null(), |s| s.0.cast_const().cast::<SECURITY_DESCRIPTOR>()),
        SecurityQualityOfService: null(),
    };
    let mut handle: HANDLE = null_mut();
    // SAFETY: plain-old-data out parameter.
    let mut status_block: IO_STATUS_BLOCK = unsafe { zeroed() };
    // SAFETY: every pointer refers to a live local for the duration of the call.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    // SAFETY: NtCreateFile succeeded, so handle is a new, owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Path-based open of an existing object, refusing a reparse point in the final component.
fn open_path(path: &Path, access: u32, share: u32, directory: bool) -> FbResult<OwnedHandle> {
    check_path(path)?;
    let name = wide(path.as_os_str());
    let flags = FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 };
    // SAFETY: name is NUL-terminated and outlives the call.
    let h = unsafe {
        CreateFileW(name.as_ptr(), access | FILE_READ_ATTRIBUTES | SYNCHRONIZE, share, null(), OPEN_EXISTING, flags, null_mut())
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: CreateFileW succeeded, so h is a new, owned handle.
    let h = unsafe { OwnedHandle::from_raw_handle(h) };
    refuse_unexpected(&h, directory)?;
    Ok(h)
}

/// Open (creating if needed) `<parent>/receipts` relative to a handle on `parent`, refuse links, foreign owners
/// and non-directories, then reset its DACL to owner-only, like `mkdirat(0o700)` + `fchmod(0o700)` on Unix.
pub(super) fn open_receipts(parent: &Path) -> FbResult<File> {
    let parent_handle = open_path(parent, FILE_LIST_DIRECTORY | FILE_TRAVERSE, DIR_SHARE, true)?;
    let security = OwnerOnly::new(true)?;
    let dir = open_at(&parent_handle, "receipts", DIR_ACCESS, DIR_SHARE, FILE_OPEN_IF, FILE_DIRECTORY_FILE, Some(&security))?;
    refuse_unexpected(&dir, true)?;
    if !owned_by_current_user(&dir)? {
        return Err(io("The receipts folder belongs to another user."));
    }
    // SAFETY: the handle has WRITE_DAC; the DACL lives as long as `security`.
    let err = unsafe {
        SetSecurityInfo(
            dir.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            security.dacl()?,
            null(),
        )
    };
    if err != ERROR_SUCCESS {
        return Err(io(std::io::Error::from_raw_os_error(err as i32)));
    }
    Ok(dir.into())
}

/// Create a new temporary file in `dir` with `CREATE_NEW` semantics and an owner-only DACL. The handle carries
/// `DELETE` so it can later be renamed into place or discarded without reopening by name.
pub(super) fn create_new(dir: &File, name: &str) -> FbResult<File> {
    let security = OwnerOnly::new(false)?;
    let h = open_at(dir, name, FILE_GENERIC_WRITE | DELETE, FILE_SHARE_NONE, FILE_CREATE, FILE_NON_DIRECTORY_FILE, Some(&security))?;
    refuse_unexpected(&h, false)?;
    Ok(h.into())
}

/// Rename the open file to `name` inside `dir`. Never replaces an existing entry of any kind (the rename fails with
/// a name collision instead), so it gives the same no-overwrite guarantee as `linkat` on Unix.
pub(super) fn rename_no_replace(file: &File, dir: &File, name: &str) -> FbResult<()> {
    if !valid_component(name) {
        return Err(io("Bad stored name."));
    }
    let wide: Vec<u16> = name.encode_utf16().collect();
    let name_offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
    let length = name_offset + wide.len() * 2;
    let mut buf = vec![0u64; length.div_ceil(8).max(size_of::<FILE_RENAME_INFORMATION>().div_ceil(8))];
    let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: buf is zeroed (ReplaceIfExists = FALSE), 8-byte aligned and large enough for the header plus name.
    unsafe {
        (*info).RootDirectory = dir.as_raw_handle() as HANDLE;
        (*info).FileNameLength = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), info.cast::<u8>().add(name_offset).cast::<u16>(), wide.len());
    }
    // SAFETY: IO_STATUS_BLOCK is plain-old-data; all-zero is a valid value.
    let mut status_block: IO_STATUS_BLOCK = unsafe { zeroed() };
    // SAFETY: info points to a buffer of `length` initialised bytes.
    let status = unsafe {
        NtSetInformationFile(file.as_raw_handle() as HANDLE, &mut status_block, info.cast(), length as u32, FileRenameInformation)
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    Ok(())
}

/// Mark an open handle for deletion; the file disappears when its last handle closes.
pub(super) fn delete_on_close(file: &File) -> FbResult<()> {
    let info = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
    // SAFETY: IO_STATUS_BLOCK is plain-old-data; all-zero is a valid value.
    let mut status_block: IO_STATUS_BLOCK = unsafe { zeroed() };
    // SAFETY: info is a live local of the declared size.
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle() as HANDLE,
            &mut status_block,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
            FileDispositionInformation,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    Ok(())
}

/// `unlinkat` for Windows: removes the entry itself (a link is removed, never its target).
pub(super) fn unlink_at(dir: &File, name: &str) -> FbResult<()> {
    let h: File = open_at(dir, name, DELETE, DIR_SHARE, FILE_OPEN, FILE_NON_DIRECTORY_FILE, None)?.into();
    delete_on_close(&h)
}

/// Open an existing stored receipt for reading, relative to the store's directory handle.
pub(super) fn open_read_at(dir: &File, name: &str) -> FbResult<File> {
    let h = open_at(dir, name, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_OPEN, FILE_NON_DIRECTORY_FILE, None)?;
    refuse_unexpected(&h, false)?;
    Ok(h.into())
}

/// Open a staged file by path for reading, refusing reparse points, devices and stream names.
pub(super) fn open_path_read(path: &Path) -> FbResult<File> {
    Ok(open_path(path, FILE_GENERIC_READ, FILE_SHARE_READ, false)?.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::storage::Store;

    #[test]
    fn reserved_and_stream_names_are_refused() {
        for bad in [
            "", ".", "..", "CON", "con", "Con.txt", "NUL", "nul.bin", "AUX", "PRN", "COM1", "com9.log", "LPT1",
            "LPT¹", "COM³", "CONIN$", "conout$", "CON .txt", "a:b", "a.bin:evil", "a.bin::$DATA", "trailing.",
            "trailing ", "a/b", "a\\b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a\u{1}b",
        ] {
            assert!(!valid_component(bad), "{bad:?} should be refused");
        }
        for good in ["receipts", ".fb-upload-0123456789abcdef", "0123456789abcdef0123456789abcdef.bin", "COM10", "CONSOLE", "LPT"] {
            assert!(valid_component(good), "{good:?} should be accepted");
        }
    }

    #[test]
    fn device_and_stream_paths_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("x.bin");
        std::fs::write(&file, b"x").unwrap();
        for bad in [
            std::path::PathBuf::from(r"\\.\NUL"),
            std::path::PathBuf::from(r"\\.\pipe\fb"),
            d.path().join("NUL"),
            d.path().join("x.bin:evil"),
            d.path().join("x.bin::$DATA"),
            d.path().join("x.bin."),
        ] {
            assert!(open_path_read(&bad).is_err(), "{bad:?} should be refused");
        }
        assert!(open_path_read(&file).is_ok());
    }

    #[test]
    fn receipts_folder_has_a_protected_owner_only_dacl() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::for_book(&d.path().join("b.fbbook"), &[1; 32]).unwrap();
        let (mut dacl, mut sd) = (null_mut::<ACL>(), null_mut());
        let err = unsafe {
            GetSecurityInfo(
                s.directory.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            )
        };
        assert_eq!(err, ERROR_SUCCESS);
        let mut control = 0u16;
        let mut revision = 0u32;
        unsafe {
            windows_sys::Win32::Security::GetSecurityDescriptorControl(sd, &mut control, &mut revision);
        }
        let aces = unsafe { (*dacl).AceCount };
        unsafe { LocalFree(sd) };
        assert_ne!(control & windows_sys::Win32::Security::SE_DACL_PROTECTED, 0);
        assert_eq!(aces, 1);
        assert!(owned_by_current_user(&s.directory).unwrap());
    }

    #[test]
    fn a_junction_in_place_of_the_receipts_folder_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(d.path().join("receipts"))
            .arg(&elsewhere)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(Store::for_book(&d.path().join("b.fbbook"), &[1; 32]).is_err());
    }

    #[test]
    fn a_file_in_place_of_the_receipts_folder_is_refused() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("receipts"), b"x").unwrap();
        assert!(Store::for_book(&d.path().join("b.fbbook"), &[1; 32]).is_err());
    }

    #[test]
    fn a_symlinked_receipt_is_never_read() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::for_book(&d.path().join("b.fbbook"), &[1; 32]).unwrap();
        let name = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.bin";
        let target = d.path().join("outside.bin");
        std::fs::write(&target, b"outside").unwrap();
        // File symlinks need Developer Mode or SeCreateSymbolicLinkPrivilege; skip where unavailable.
        if std::os::windows::fs::symlink_file(&target, s.folder().join(name)).is_err() {
            return;
        }
        assert!(open_read_at(&s.directory, name).is_err());
        assert!(open_path_read(&s.folder().join(name)).is_err());
    }

    #[test]
    fn failed_placement_leaves_no_temporary_file() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::for_book(&d.path().join("b.fbbook"), &[1; 32]).unwrap();
        let name = "cccccccccccccccccccccccccccccccc.bin";
        s.place_chunks(name, std::iter::once(Ok(b"one".to_vec()))).unwrap();
        assert!(s.place_chunks(name, std::iter::once(Err(io("boom")))).is_err());
        assert!(s.place_chunks(name, std::iter::once(Ok(b"two".to_vec()))).is_err());
        let names: Vec<_> = std::fs::read_dir(s.folder()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from(name)]);
        assert_eq!(std::fs::read(s.folder().join(name)).unwrap(), b"one");
        s.remove_unreferenced(name);
        assert_eq!(std::fs::read_dir(s.folder()).unwrap().count(), 0);
    }
}
