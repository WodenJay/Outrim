use std::{
    io,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, LocalFree},
    Security::{
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
            SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, GetVolumeInformationByHandleW,
        READ_CONTROL, WRITE_DAC,
    },
    System::SystemServices::FILE_PERSISTENT_ACLS,
};

struct Descriptor(*mut std::ffi::c_void);

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW allocates
        // this descriptor with LocalAlloc; this guard is its sole owner.
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(crate) fn private_directory(path: &Path) -> io::Result<()> {
    let path_wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if path_wide[..path_wide.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Original path contains NUL",
        ));
    }
    // A protected DACL with inheritable owner-only full access. Supplying it at
    // creation avoids a window in which a new directory inherits broader rights.
    let sddl: Vec<_> = "D:P(A;OICI;FA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut raw = ptr::null_mut();
    // SAFETY: input is NUL-terminated; output points to a live pointer slot.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut raw,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = Descriptor(raw);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: path and descriptor remain alive for the entire call.
    if unsafe { CreateDirectoryW(path_wide.as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(error);
        }
    }
    use std::os::windows::fs::MetadataExt;
    let directory = std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(format!(
            "Not a plain directory: {}",
            path.display()
        )));
    }
    let mut flags = 0;
    // SAFETY: directory owns a valid handle; unused outputs are null.
    if unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut flags,
            ptr::null_mut(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(io::Error::other(format!(
            "Volume does not support access control: {}",
            path.display()
        )));
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = ptr::null_mut();
    // SAFETY: descriptor is valid; output slots live until the call returns.
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    // Reapply to existing directories as well; inherited broad permissions must
    // not leak into newly created originals. Never modify ownership or SACLs.
    // SAFETY: directory and DACL are valid and live; null owner/group/SACL are unused.
    let error = unsafe {
        SetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}
