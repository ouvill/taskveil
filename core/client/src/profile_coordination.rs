use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetHandleInformation, LocalFree, SetHandleInformation,
        ERROR_CALL_NOT_IMPLEMENTED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_FUNCTION,
        ERROR_NOT_SUPPORTED, ERROR_SUCCESS, GENERIC_ALL, GENERIC_WRITE, HANDLE,
        HANDLE_FLAG_INHERIT, HLOCAL,
    },
    Security::{
        AccessCheck,
        Authorization::{
            BuildTrusteeWithSidW, GetEffectiveRightsFromAclW, GetExplicitEntriesFromAclW,
            GetSecurityInfo, EXPLICIT_ACCESS_W, SE_FILE_OBJECT, TRUSTEE_IS_OBJECTS_AND_SID,
            TRUSTEE_IS_SID,
        },
        CreateWellKnownSid, DuplicateToken, EqualSid, GetAce, GetLengthSid, GetTokenInformation,
        IsValidSid, SecurityImpersonation, TokenUser, WinBuiltinAdministratorsSid,
        WinCreatorOwnerRightsSid, WinCreatorOwnerSid, WinLocalSystemSid, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, GENERIC_MAPPING, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_MAX_SID_SIZE,
        TOKEN_DUPLICATE, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ADD_FILE,
        FILE_ADD_SUBDIRECTORY, FILE_ALL_ACCESS, FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
        READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::ClientError;

#[cfg(any(windows, test))]
const PROFILE_LOCK_FILE_NAME: &str = ".taskveil-profile.lock";
const SESSION_LOCK_FILE_NAME: &str = ".taskveil-session-token-set.lock";
#[cfg(windows)]
const WINDOWS_LOCK_OPEN_RETRIES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProfileIdentity {
    volume_serial_or_device: u64,
    file_index_or_inode: u64,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct WindowsHandleInfo {
    identity: ProfileIdentity,
    attributes: u32,
    link_count: u32,
}

struct ProfileLockHandles {
    lock: File,
    session_lock: File,
    #[cfg(unix)]
    session_identity: ProfileIdentity,
    #[cfg(windows)]
    _profile_root: File,
}

static PROCESS_COORDINATORS: OnceLock<Mutex<HashMap<ProfileIdentity, Weak<ProfileCoordinator>>>> =
    OnceLock::new();

pub(crate) struct ProfileCoordinator {
    identity: ProfileIdentity,
    canonical_root: PathBuf,
    lock_handles: ProfileLockHandles,
    database_identity: Mutex<Option<ProfileIdentity>>,
    process_lock: Mutex<ProcessLockState>,
    session_lock: Mutex<SessionLockState>,
}

#[derive(Default)]
struct ProcessLockState {
    readers: usize,
    writer: bool,
    poisoned: bool,
}

#[derive(Default)]
struct SessionLockState {
    held: bool,
    poisoned: bool,
}

impl ProfileCoordinator {
    pub(crate) fn for_profile(db_dir: &Path) -> Result<Arc<Self>, ClientError> {
        std::fs::create_dir_all(db_dir).map_err(ClientError::Io)?;
        let canonical_root = db_dir
            .canonicalize()
            .map_err(|_| ClientError::ProfileLockUnsupported)?;
        let (identity, lock_handles) = Self::open_lock_handles(&canonical_root)?;
        let registry = PROCESS_COORDINATORS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut registry = registry.lock().map_err(|_| ClientError::RuntimeState)?;
        registry.retain(|_, coordinator| coordinator.strong_count() != 0);
        if let Some(existing) = registry.get(&identity).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        let coordinator = Arc::new(Self {
            identity,
            canonical_root,
            lock_handles,
            database_identity: Mutex::new(None),
            process_lock: Mutex::new(ProcessLockState::default()),
            session_lock: Mutex::new(SessionLockState::default()),
        });
        registry.insert(identity, Arc::downgrade(&coordinator));
        Ok(coordinator)
    }

    pub(crate) fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub(crate) fn pin_database(&self, db_path: &Path) -> Result<(), ClientError> {
        let identity = validate_profile_database_path(db_path)?;
        let mut pinned = self
            .database_identity
            .lock()
            .map_err(|_| ClientError::RuntimeState)?;
        match *pinned {
            Some(expected) if expected != identity => Err(ClientError::ProfileLockUnsupported),
            Some(_) => Ok(()),
            None => {
                *pinned = Some(identity);
                Ok(())
            }
        }
    }

    pub(crate) fn validate_pinned_paths(&self, db_path: &Path) -> Result<(), ClientError> {
        validate_profile_root_path(&self.canonical_root, self.identity)?;
        let expected = self
            .database_identity
            .lock()
            .map_err(|_| ClientError::RuntimeState)?
            .ok_or(ClientError::ProfileLockUnsupported)?;
        if validate_profile_database_path(db_path)? != expected {
            return Err(ClientError::ProfileLockUnsupported);
        }
        Ok(())
    }

    pub(crate) fn try_shared(self: &Arc<Self>) -> Result<ProfileSharedGuard, ClientError> {
        let mut process = self
            .process_lock
            .lock()
            .map_err(|_| ClientError::RuntimeState)?;
        if process.poisoned {
            return Err(ClientError::ProfileLockUnsupported);
        }
        if process.writer {
            return Err(ClientError::ProfileBusy);
        }
        if process.readers == 0 {
            try_lock_shared(&self.lock_handles.lock)?;
        }
        process.readers = process
            .readers
            .checked_add(1)
            .ok_or(ClientError::RuntimeState)?;
        Ok(ProfileSharedGuard {
            coordinator: Arc::clone(self),
        })
    }

    pub(crate) fn try_exclusive(self: &Arc<Self>) -> Result<ProfileExclusiveGuard, ClientError> {
        let mut process = self
            .process_lock
            .lock()
            .map_err(|_| ClientError::RuntimeState)?;
        if process.poisoned {
            return Err(ClientError::ProfileLockUnsupported);
        }
        if process.writer || process.readers != 0 {
            return Err(ClientError::ProfileBusy);
        }
        try_lock_exclusive(&self.lock_handles.lock)?;
        process.writer = true;
        Ok(ProfileExclusiveGuard {
            coordinator: Arc::clone(self),
        })
    }

    pub(crate) fn try_session(self: &Arc<Self>) -> Result<SessionCredentialGuard, ClientError> {
        let mut state = self
            .session_lock
            .lock()
            .map_err(|_| ClientError::RuntimeState)?;
        if state.poisoned {
            return Err(ClientError::ProfileLockUnsupported);
        }
        if state.held {
            return Err(ClientError::Busy);
        }
        #[cfg(unix)]
        verify_unix_anchored_lock_file(
            &self.lock_handles.lock,
            SESSION_LOCK_FILE_NAME,
            self.lock_handles.session_identity,
        )?;
        match try_lock_exclusive(&self.lock_handles.session_lock) {
            Ok(()) => {
                state.held = true;
                Ok(SessionCredentialGuard {
                    coordinator: Arc::clone(self),
                })
            }
            Err(ClientError::ProfileBusy) => Err(ClientError::Busy),
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    fn open_lock_handles(
        canonical_root: &Path,
    ) -> Result<(ProfileIdentity, ProfileLockHandles), ClientError> {
        let handle = File::open(canonical_root).map_err(ClientError::Io)?;
        let metadata = handle.metadata().map_err(ClientError::Io)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(ClientError::ProfileLockUnsupported);
        }
        let session_lock = open_unix_anchored_lock_file(&handle, SESSION_LOCK_FILE_NAME)?;
        let session_metadata = session_lock
            .metadata()
            .map_err(|_| ClientError::ProfileLockUnsupported)?;
        Ok((
            ProfileIdentity {
                volume_serial_or_device: metadata.dev(),
                file_index_or_inode: metadata.ino(),
            },
            ProfileLockHandles {
                lock: handle,
                session_lock,
                session_identity: ProfileIdentity {
                    volume_serial_or_device: session_metadata.dev(),
                    file_index_or_inode: session_metadata.ino(),
                },
            },
        ))
    }

    #[cfg(windows)]
    fn open_lock_handles(
        canonical_root: &Path,
    ) -> Result<(ProfileIdentity, ProfileLockHandles), ClientError> {
        let root_handle = open_windows_directory_handle(canonical_root)?;
        let root_info = windows_handle_info(&root_handle)?;
        validate_windows_directory_info(root_info)?;
        validate_windows_profile_security(&root_handle)?;

        let root_verifier = open_windows_directory_handle(canonical_root)?;
        let verified_root_info = windows_handle_info(&root_verifier)?;
        validate_windows_directory_info(verified_root_info)?;
        if root_info.identity != verified_root_info.identity {
            return Err(ClientError::ProfileLockUnsupported);
        }

        let lock_handle = open_windows_lock_file(&canonical_root.join(PROFILE_LOCK_FILE_NAME))?;
        let session_lock = open_windows_lock_file(&canonical_root.join(SESSION_LOCK_FILE_NAME))?;
        Ok((
            root_info.identity,
            ProfileLockHandles {
                lock: lock_handle,
                session_lock,
                _profile_root: root_handle,
            },
        ))
    }

    fn release_shared(&self) {
        let mut state = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.readers {
            0 => state.poisoned = true,
            1 => {
                if unlock_file(&self.lock_handles.lock).is_err() {
                    state.poisoned = true;
                } else {
                    state.readers = 0;
                }
            }
            _ => state.readers -= 1,
        }
    }

    fn release_exclusive(&self) {
        let mut state = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.writer {
            state.poisoned = true;
            return;
        }
        if unlock_file(&self.lock_handles.lock).is_err() {
            state.poisoned = true;
        } else {
            state.writer = false;
        }
    }

    fn release_session(&self) {
        let mut state = self
            .session_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.held {
            state.poisoned = true;
            return;
        }
        if unlock_file(&self.lock_handles.session_lock).is_err() {
            state.poisoned = true;
        } else {
            state.held = false;
        }
    }
}

#[cfg(unix)]
fn validate_profile_root_path(path: &Path, expected: ProfileIdentity) -> Result<(), ClientError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ClientError::ProfileLockUnsupported)?;
    let identity = ProfileIdentity {
        volume_serial_or_device: metadata.dev(),
        file_index_or_inode: metadata.ino(),
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
        || identity != expected
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(unix)]
fn validate_profile_database_path(path: &Path) -> Result<ProfileIdentity, ClientError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ClientError::ProfileLockUnsupported)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o022 != 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(ProfileIdentity {
        volume_serial_or_device: metadata.dev(),
        file_index_or_inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn validate_profile_root_path(path: &Path, expected: ProfileIdentity) -> Result<(), ClientError> {
    let root = open_windows_directory_handle(path)?;
    let info = windows_handle_info(&root)?;
    validate_windows_directory_info(info)?;
    validate_windows_profile_security(&root)?;
    if info.identity != expected {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
fn validate_profile_database_path(path: &Path) -> Result<ProfileIdentity, ClientError> {
    let database = open_windows_inspection_handle(path)?;
    let info = windows_handle_info(&database)?;
    validate_windows_lock_info(info)?;
    validate_windows_file_security(&database)?;
    Ok(info.identity)
}

#[cfg(unix)]
fn open_unix_anchored_lock_file(profile_root: &File, name: &str) -> Result<File, ClientError> {
    use std::{ffi::CString, os::fd::FromRawFd};

    let name = CString::new(name).map_err(|_| ClientError::ProfileLockUnsupported)?;
    let fd = unsafe {
        libc::openat(
            profile_root.as_raw_fd(),
            name.as_ptr(),
            libc::O_CLOEXEC | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_RDWR,
            0o600,
        )
    };
    if fd < 0 {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|_| ClientError::ProfileLockUnsupported)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(file)
}

#[cfg(unix)]
fn verify_unix_anchored_lock_file(
    profile_root: &File,
    name: &str,
    expected: ProfileIdentity,
) -> Result<(), ClientError> {
    use std::{ffi::CString, os::fd::FromRawFd};

    let name = CString::new(name).map_err(|_| ClientError::ProfileLockUnsupported)?;
    let fd = unsafe {
        libc::openat(
            profile_root.as_raw_fd(),
            name.as_ptr(),
            libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_RDWR,
        )
    };
    if fd < 0 {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let verifier = unsafe { File::from_raw_fd(fd) };
    let metadata = verifier
        .metadata()
        .map_err(|_| ClientError::ProfileLockUnsupported)?;
    let identity = ProfileIdentity {
        volume_serial_or_device: metadata.dev(),
        file_index_or_inode: metadata.ino(),
    };
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || identity != expected
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
fn open_windows_directory_handle(path: &Path) -> Result<File, ClientError> {
    let mut options = OpenOptions::new();
    options
        .access_mode(READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options
        .open(path)
        .map_err(|_| ClientError::ProfileLockUnsupported)?;
    make_windows_handle_non_inheritable(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn open_windows_lock_file(path: &Path) -> Result<File, ClientError> {
    for _ in 0..WINDOWS_LOCK_OPEN_RETRIES {
        match open_windows_inspection_handle(path) {
            Ok(inspector) => {
                let inspected = windows_handle_info(&inspector)?;
                validate_windows_lock_info(inspected)?;
                let file = open_windows_read_write_handle(path, false)?;
                let opened = windows_handle_info(&file)?;
                validate_windows_lock_info(opened)?;
                if inspected.identity != opened.identity {
                    return Err(ClientError::ProfileLockUnsupported);
                }
                validate_windows_file_security(&file)?;
                return Ok(file);
            }
            Err(ClientError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                match open_windows_read_write_handle(path, true) {
                    Ok(file) => {
                        let opened = windows_handle_info(&file)?;
                        validate_windows_lock_info(opened)?;
                        let verifier = open_windows_inspection_handle(path)?;
                        let verified = windows_handle_info(&verifier)?;
                        validate_windows_lock_info(verified)?;
                        if opened.identity != verified.identity {
                            return Err(ClientError::ProfileLockUnsupported);
                        }
                        validate_windows_file_security(&file)?;
                        return Ok(file);
                    }
                    Err(ClientError::Io(error))
                        if error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(ClientError::ProfileLockUnsupported)
}

#[cfg(windows)]
fn open_windows_inspection_handle(path: &Path) -> Result<File, ClientError> {
    let mut options = OpenOptions::new();
    options
        .access_mode(READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).map_err(map_windows_open_error)?;
    make_windows_handle_non_inheritable(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn open_windows_read_write_handle(path: &Path, create_new: bool) -> Result<File, ClientError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if create_new {
        options.create_new(true);
    }
    let file = options.open(path).map_err(map_windows_open_error)?;
    make_windows_handle_non_inheritable(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn make_windows_handle_non_inheritable(file: &File) -> Result<(), ClientError> {
    if unsafe { SetHandleInformation(file.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, 0) } == 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let mut flags = 0;
    if unsafe { GetHandleInformation(file.as_raw_handle() as HANDLE, &mut flags) } == 0
        || flags & HANDLE_FLAG_INHERIT != 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
struct OwnedWindowsHandle(HANDLE);

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
struct LocalAllocation(HLOCAL);

#[cfg(windows)]
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn validate_windows_profile_security(profile_root: &File) -> Result<(), ClientError> {
    let desired =
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD;
    validate_windows_object_security(profile_root, desired, true)
}

#[cfg(windows)]
fn validate_windows_file_security(file: &File) -> Result<(), ClientError> {
    validate_windows_object_security(file, FILE_GENERIC_READ | FILE_GENERIC_WRITE, false)
}

#[cfg(windows)]
fn validate_windows_object_security(
    object: &File,
    desired_current_access: u32,
    inspect_child_inheritance: bool,
) -> Result<(), ClientError> {
    use windows_sys::Win32::Security::Authorization::{OBJECTS_AND_SID, TRUSTEE_W};

    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            object.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::addr_of_mut!(owner),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(dacl),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(descriptor),
        )
    };
    if status != ERROR_SUCCESS || descriptor.is_null() || owner.is_null() || dacl.is_null() {
        return Err(windows_security_failure("security_descriptor"));
    }
    let _descriptor = LocalAllocation(descriptor);

    let process_token = open_current_process_token()?;
    let token_user_buffer = token_user_buffer(process_token.0)?;
    let token_user = unsafe { (*token_user_buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if token_user.is_null() {
        return Err(windows_security_failure("token_user"));
    }

    let local_system_sid = well_known_sid(WinLocalSystemSid)?;
    let administrators_sid = well_known_sid(WinBuiltinAdministratorsSid)?;
    // Windows uses the token's default owner for newly created objects. For an
    // elevated member of the built-in Administrators group that owner is
    // commonly BUILTIN\Administrators rather than TokenUser. Treat the two
    // machine-wide principals that are already inside our trusted boundary as
    // valid owners, while the DACL checks below still reject every untrusted
    // writer.
    if unsafe { EqualSid(owner, token_user) } == 0
        && unsafe { EqualSid(owner, local_system_sid.as_ptr().cast_mut().cast()) } == 0
        && unsafe { EqualSid(owner, administrators_sid.as_ptr().cast_mut().cast()) } == 0
    {
        return Err(windows_security_failure("owner"));
    }
    verify_current_token_access(descriptor, process_token.0, desired_current_access)?;

    let trusted_sids = [
        local_system_sid,
        administrators_sid,
        well_known_sid(WinCreatorOwnerSid)?,
        well_known_sid(WinCreatorOwnerRightsSid)?,
    ];
    let mut entry_count = 0;
    let mut entries: *mut EXPLICIT_ACCESS_W = std::ptr::null_mut();
    let status = unsafe { GetExplicitEntriesFromAclW(dacl, &mut entry_count, &mut entries) };
    if status != ERROR_SUCCESS || (entry_count != 0 && entries.is_null()) {
        return Err(windows_security_failure("acl_entries"));
    }
    let _entries = LocalAllocation(entries.cast());
    let dangerous = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | FILE_ADD_FILE
        | FILE_ADD_SUBDIRECTORY
        | FILE_DELETE_CHILD
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_WRITE
        | GENERIC_ALL;
    reject_untrusted_raw_write_aces(
        dacl,
        token_user,
        &trusted_sids,
        dangerous,
        inspect_child_inheritance,
    )?;
    for entry in unsafe { std::slice::from_raw_parts(entries, entry_count as usize) } {
        if !entry.Trustee.pMultipleTrustee.is_null() {
            return Err(windows_security_failure("multiple_trustee"));
        }
        let sid: PSID = match entry.Trustee.TrusteeForm {
            TRUSTEE_IS_SID => entry.Trustee.ptstrName.cast(),
            TRUSTEE_IS_OBJECTS_AND_SID => {
                let object = entry.Trustee.ptstrName.cast::<OBJECTS_AND_SID>();
                if object.is_null() {
                    return Err(windows_security_failure("object_trustee"));
                }
                unsafe { (*object).pSid.cast() }
            }
            _ => return Err(windows_security_failure("trustee_form")),
        };
        if sid.is_null() {
            return Err(windows_security_failure("trustee_sid"));
        }
        if unsafe { EqualSid(sid, token_user) } != 0
            || trusted_sids
                .iter()
                .any(|trusted| unsafe { EqualSid(sid, trusted.as_ptr().cast_mut().cast()) } != 0)
        {
            continue;
        }
        let mut trustee = TRUSTEE_W::default();
        unsafe {
            BuildTrusteeWithSidW(&mut trustee, sid);
        }
        let mut effective = 0;
        let status = unsafe { GetEffectiveRightsFromAclW(dacl, &trustee, &mut effective) };
        if status != ERROR_SUCCESS || effective & dangerous != 0 {
            return Err(windows_security_failure("untrusted_effective_write"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn reject_untrusted_raw_write_aces(
    dacl: *mut windows_sys::Win32::Security::ACL,
    token_user: PSID,
    trusted_sids: &[Vec<u8>],
    dangerous: u32,
    inspect_child_inheritance: bool,
) -> Result<(), ClientError> {
    // GetEffectiveRightsFromAcl evaluates access to the object itself, while
    // INHERIT_ONLY allow ACEs intentionally do not contribute to that result.
    // Inspect raw allow ACEs both as a second check on existing files and, for
    // the root directory, as a fail-closed child-inheritance boundary.
    let ace_count = unsafe { (*dacl).AceCount };
    for index in 0..u32::from(ace_count) {
        let mut raw_ace = std::ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 || raw_ace.is_null() {
            return Err(windows_security_failure("raw_ace"));
        }
        let header = unsafe {
            std::ptr::read_unaligned(raw_ace.cast::<windows_sys::Win32::Security::ACE_HEADER>())
        };
        let inherited_to_children =
            u32::from(header.AceFlags) & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) != 0;
        let applies_to_object = u32::from(header.AceFlags) & INHERIT_ONLY_ACE == 0;
        if !applies_to_object && !(inspect_child_inheritance && inherited_to_children) {
            continue;
        }
        let ace_size = usize::from(header.AceSize);
        if ace_size < std::mem::size_of::<windows_sys::Win32::Security::ACE_HEADER>() + 4 {
            return Err(windows_security_failure("raw_ace_size"));
        }
        let mask = unsafe {
            std::ptr::read_unaligned(
                raw_ace
                    .cast::<u8>()
                    .add(std::mem::size_of::<windows_sys::Win32::Security::ACE_HEADER>())
                    .cast::<u32>(),
            )
        };
        if mask & dangerous == 0 {
            continue;
        }

        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
        const ACCESS_ALLOWED_OBJECT_ACE_TYPE: u8 = 5;
        const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 9;
        const ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE: u8 = 11;
        const ACE_OBJECT_TYPE_PRESENT: u32 = 1;
        const ACE_INHERITED_OBJECT_TYPE_PRESENT: u32 = 2;

        let sid_offset = match header.AceType {
            ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => 8,
            ACCESS_ALLOWED_OBJECT_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE => {
                if ace_size < 12 {
                    return Err(windows_security_failure("object_ace_size"));
                }
                let object_flags =
                    unsafe { std::ptr::read_unaligned(raw_ace.cast::<u8>().add(8).cast::<u32>()) };
                12 + usize::from(object_flags & ACE_OBJECT_TYPE_PRESENT != 0) * 16
                    + usize::from(object_flags & ACE_INHERITED_OBJECT_TYPE_PRESENT != 0) * 16
            }
            // Compound allow ACEs are not supported by AccessCheck on current
            // Windows versions, and their two-SID layout differs.
            4 => return Err(windows_security_failure("compound_allow_ace")),
            // Known deny, audit, alarm, label, resource, policy, and trust ACEs
            // do not grant access. An unknown inheritable ACE with a dangerous
            // mask fails closed because its grant semantics/layout are unknown.
            1..=3 | 6..=8 | 10 | 12..=21 => continue,
            _ => return Err(windows_security_failure("unknown_allow_ace")),
        };
        if sid_offset >= ace_size {
            return Err(windows_security_failure("raw_sid_offset"));
        }
        let sid: PSID = unsafe { raw_ace.cast::<u8>().add(sid_offset).cast() };
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            return Err(windows_security_failure("raw_sid"));
        }
        let sid_length = unsafe { GetLengthSid(sid) } as usize;
        if sid_length == 0 || sid_offset.saturating_add(sid_length) > ace_size {
            return Err(windows_security_failure("raw_sid_size"));
        }
        if unsafe { EqualSid(sid, token_user) } != 0
            || trusted_sids
                .iter()
                .any(|trusted| unsafe { EqualSid(sid, trusted.as_ptr().cast_mut().cast()) } != 0)
        {
            continue;
        }
        return Err(windows_security_failure("untrusted_raw_write"));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_security_failure(reason: &'static str) -> ClientError {
    #[cfg(test)]
    eprintln!("taskveil windows profile security rejection: {reason}");
    #[cfg(not(test))]
    let _ = reason;
    ClientError::ProfileLockUnsupported
}

#[cfg(windows)]
fn open_current_process_token() -> Result<OwnedWindowsHandle, ClientError> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )
    } == 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(OwnedWindowsHandle(token))
}

#[cfg(windows)]
fn token_user_buffer(token: HANDLE) -> Result<Vec<usize>, ClientError> {
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0
        || std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0usize; words];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if sid.is_null() {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let sid_length = unsafe { windows_sys::Win32::Security::GetLengthSid(sid) };
    if sid_length == 0 || sid_length > SECURITY_MAX_SID_SIZE {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(buffer)
}

#[cfg(windows)]
fn well_known_sid(kind: i32) -> Result<Vec<u8>, ClientError> {
    let mut size = SECURITY_MAX_SID_SIZE;
    let mut sid = vec![0u8; size as usize];
    if unsafe {
        CreateWellKnownSid(
            kind,
            std::ptr::null_mut(),
            sid.as_mut_ptr().cast(),
            &mut size,
        )
    } == 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    sid.truncate(size as usize);
    Ok(sid)
}

#[cfg(windows)]
fn verify_current_token_access(
    descriptor: PSECURITY_DESCRIPTOR,
    primary_token: HANDLE,
    desired: u32,
) -> Result<(), ClientError> {
    let mut impersonation_token: HANDLE = std::ptr::null_mut();
    if unsafe {
        DuplicateToken(
            primary_token,
            SecurityImpersonation,
            &mut impersonation_token,
        )
    } == 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    let impersonation_token = OwnedWindowsHandle(impersonation_token);
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut privileges = vec![0usize; 4096 / std::mem::size_of::<usize>()];
    let mut privileges_len = (privileges.len() * std::mem::size_of::<usize>()) as u32;
    let mut granted = 0;
    let mut allowed = 0;
    if unsafe {
        AccessCheck(
            descriptor,
            impersonation_token.0,
            desired,
            &mapping,
            privileges.as_mut_ptr().cast(),
            &mut privileges_len,
            &mut granted,
            &mut allowed,
        )
    } == 0
        || allowed == 0
        || granted & desired != desired
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
fn windows_handle_info(file: &File) -> Result<WindowsHandleInfo, ClientError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let result = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as HANDLE, std::ptr::addr_of_mut!(info))
    };
    if result == 0 {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(WindowsHandleInfo {
        identity: ProfileIdentity {
            volume_serial_or_device: u64::from(info.dwVolumeSerialNumber),
            file_index_or_inode: (u64::from(info.nFileIndexHigh) << 32)
                | u64::from(info.nFileIndexLow),
        },
        attributes: info.dwFileAttributes,
        link_count: info.nNumberOfLinks,
    })
}

#[cfg(windows)]
fn validate_windows_directory_info(info: WindowsHandleInfo) -> Result<(), ClientError> {
    if info.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_lock_info(info: WindowsHandleInfo) -> Result<(), ClientError> {
    if info.attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || info.link_count != 1
    {
        return Err(ClientError::ProfileLockUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
fn map_windows_open_error(error: std::io::Error) -> ClientError {
    if is_unsupported_lock_error(&error) {
        ClientError::ProfileLockUnsupported
    } else {
        ClientError::Io(error)
    }
}

pub(crate) struct ProfileSharedGuard {
    coordinator: Arc<ProfileCoordinator>,
}

pub(crate) struct ProfileExclusiveGuard {
    coordinator: Arc<ProfileCoordinator>,
}

pub(crate) struct SessionCredentialGuard {
    coordinator: Arc<ProfileCoordinator>,
}

impl Drop for ProfileSharedGuard {
    fn drop(&mut self) {
        self.coordinator.release_shared();
    }
}

impl Drop for ProfileExclusiveGuard {
    fn drop(&mut self) {
        self.coordinator.release_exclusive();
    }
}

impl Drop for SessionCredentialGuard {
    fn drop(&mut self) {
        self.coordinator.release_session();
    }
}

#[cfg(not(target_os = "android"))]
fn try_lock_shared(file: &File) -> Result<(), ClientError> {
    match file.try_lock_shared() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(ClientError::ProfileBusy),
        Err(std::fs::TryLockError::Error(error)) if is_unsupported_lock_error(&error) => {
            Err(ClientError::ProfileLockUnsupported)
        }
        Err(std::fs::TryLockError::Error(error)) => Err(ClientError::Io(error)),
    }
}

#[cfg(not(target_os = "android"))]
fn try_lock_exclusive(file: &File) -> Result<(), ClientError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(ClientError::ProfileBusy),
        Err(std::fs::TryLockError::Error(error)) if is_unsupported_lock_error(&error) => {
            Err(ClientError::ProfileLockUnsupported)
        }
        Err(std::fs::TryLockError::Error(error)) => Err(ClientError::Io(error)),
    }
}

#[cfg(unix)]
fn is_unsupported_lock_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::Unsupported
        || matches!(
            error.raw_os_error(),
            Some(code) if code == libc::ENOSYS || code == libc::EOPNOTSUPP
        )
}

#[cfg(windows)]
fn is_unsupported_lock_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::Unsupported
        || matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_INVALID_FUNCTION as i32
                    || code == ERROR_NOT_SUPPORTED as i32
                    || code == ERROR_CALL_NOT_IMPLEMENTED as i32
        )
}

#[cfg(not(any(unix, windows)))]
fn is_unsupported_lock_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::Unsupported
}

#[cfg(test)]
std::thread_local! {
    static FORCE_UNLOCK_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn take_forced_unlock_failure() -> bool {
    FORCE_UNLOCK_FAILURE.with(|forced| forced.replace(false))
}

#[cfg(not(target_os = "android"))]
fn unlock_file(file: &File) -> Result<(), ClientError> {
    #[cfg(test)]
    if take_forced_unlock_failure() {
        return Err(ClientError::ProfileLockUnsupported);
    }
    file.unlock().map_err(ClientError::Io)
}

#[cfg(target_os = "android")]
fn try_lock_shared(file: &File) -> Result<(), ClientError> {
    android_flock(file, libc::LOCK_SH | libc::LOCK_NB)
}

#[cfg(target_os = "android")]
fn try_lock_exclusive(file: &File) -> Result<(), ClientError> {
    android_flock(file, libc::LOCK_EX | libc::LOCK_NB)
}

#[cfg(target_os = "android")]
fn android_flock(file: &File, operation: libc::c_int) -> Result<(), ClientError> {
    let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => {
            Err(ClientError::ProfileBusy)
        }
        Some(code) if code == libc::ENOSYS || code == libc::EOPNOTSUPP => {
            Err(ClientError::ProfileLockUnsupported)
        }
        _ => Err(ClientError::Io(error)),
    }
}

#[cfg(target_os = "android")]
fn unlock_file(file: &File) -> Result<(), ClientError> {
    #[cfg(test)]
    if take_forced_unlock_failure() {
        return Err(ClientError::ProfileLockUnsupported);
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(ClientError::Io(std::io::Error::last_os_error()))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };

    use taskveil_domain::{new_list, new_task};
    use taskveil_storage::{
        open_encrypted, ListRepository, SqliteListRepository, SqliteSyncStateRepository,
        SqliteTaskRepository, SyncStateRepository, TaskRepository,
    };
    use taskveil_sync::LocalSyncKeys;
    use tempfile::TempDir;
    use zeroize::Zeroizing;

    use super::*;
    use crate::{LocalMutationContext, SqliteMutationService, SqliteSyncStore, UpdateTaskInput};

    const PROCESS_TEST_DB_KEY: [u8; 32] = [0xa7; 32];

    #[test]
    fn canonical_aliases_share_process_coordinator() {
        let temp = TempDir::new().unwrap();
        #[cfg(unix)]
        let root = temp.path().join("profile");
        #[cfg(windows)]
        let root = temp.path().join("ProfileCase");
        std::fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        let alias = temp.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        #[cfg(windows)]
        let alias = temp.path().join("pRoFiLeCaSe");

        let first = ProfileCoordinator::for_profile(&root).unwrap();
        let second = ProfileCoordinator::for_profile(&alias).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let _exclusive = first.try_exclusive().unwrap();
        assert!(matches!(second.try_shared(), Err(ClientError::ProfileBusy)));
    }

    #[test]
    fn canonical_path_aliases_share_the_same_process_lock() {
        let temp = TempDir::new().unwrap();
        #[cfg(unix)]
        let profile = temp.path().join("profile");
        #[cfg(windows)]
        let profile = temp.path().join("ProfileCase");
        std::fs::create_dir(&profile).unwrap();
        #[cfg(unix)]
        let alias = {
            let alias = temp.path().join("profile-alias");
            std::os::unix::fs::symlink(&profile, &alias).unwrap();
            alias
        };
        #[cfg(windows)]
        let alias = temp.path().join("pRoFiLeCaSe");

        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let _guard = coordinator.try_exclusive().unwrap();
        let probe = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_exclusive_lock_probe",
                "--nocapture",
            ])
            .env("TASKVEIL_PROFILE_LOCK_PROBE", &alias)
            .status()
            .unwrap();
        assert_eq!(probe.code(), Some(42));
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_alias_shares_the_same_real_process_lock() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let alias = temp.path().join("profile-junction");
        std::fs::create_dir(&profile).unwrap();
        assert!(Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&profile)
            .status()
            .unwrap()
            .success());

        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let _guard = coordinator.try_exclusive().unwrap();
        let probe = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_exclusive_lock_probe",
                "--nocapture",
            ])
            .env("TASKVEIL_PROFILE_LOCK_PROBE", &alias)
            .status()
            .unwrap();
        assert_eq!(probe.code(), Some(42));
    }

    #[test]
    fn different_profiles_do_not_contend() {
        let temp = TempDir::new().unwrap();
        let first = ProfileCoordinator::for_profile(&temp.path().join("one")).unwrap();
        let second = ProfileCoordinator::for_profile(&temp.path().join("two")).unwrap();
        let _first = first.try_exclusive().unwrap();
        let _second = second.try_exclusive().unwrap();
    }

    #[test]
    fn different_profiles_do_not_contend_across_real_processes() {
        let temp = TempDir::new().unwrap();
        let first_profile = temp.path().join("one");
        let second_profile = temp.path().join("two");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_profile_lock_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_PROFILE_LOCK_CHILD", &first_profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_PROFILE_LOCK_READY") {
                break;
            }
        }

        let second = ProfileCoordinator::for_profile(&second_profile).unwrap();
        let _second_guard = second.try_exclusive().unwrap();

        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn session_sublock_excludes_same_process_instances() {
        let temp = TempDir::new().unwrap();
        let coordinator = ProfileCoordinator::for_profile(&temp.path().join("profile")).unwrap();
        let first = coordinator.try_session().unwrap();
        assert!(matches!(coordinator.try_session(), Err(ClientError::Busy)));
        drop(first);
        coordinator.try_session().unwrap();
    }

    #[test]
    fn coordinator_handles_are_explicitly_close_on_exec() {
        let temp = TempDir::new().unwrap();
        let coordinator = ProfileCoordinator::for_profile(&temp.path().join("profile")).unwrap();
        #[cfg(unix)]
        for file in [
            &coordinator.lock_handles.lock,
            &coordinator.lock_handles.session_lock,
        ] {
            let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
            assert_ne!(flags, -1);
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }
        #[cfg(windows)]
        for file in [
            &coordinator.lock_handles.lock,
            &coordinator.lock_handles.session_lock,
            &coordinator.lock_handles._profile_root,
        ] {
            let mut flags = 0;
            assert_ne!(
                unsafe { GetHandleInformation(file.as_raw_handle() as HANDLE, &mut flags) },
                0
            );
            assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
        }
    }

    #[test]
    fn unlock_failure_permanently_poisons_the_coordinator() {
        let temp = TempDir::new().unwrap();
        let coordinator = ProfileCoordinator::for_profile(&temp.path().join("profile")).unwrap();
        let guard = coordinator.try_exclusive().unwrap();
        FORCE_UNLOCK_FAILURE.with(|forced| forced.set(true));
        drop(guard);
        assert!(matches!(
            coordinator.try_shared(),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn replaceable_child_lockfile_is_not_the_lock_authority() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let target = temp.path().join("unrelated");
        File::create(&target).unwrap();
        std::os::unix::fs::symlink(&target, profile.join(PROFILE_LOCK_FILE_NAME)).unwrap();
        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let _guard = coordinator.try_exclusive().unwrap();
        let probe = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_exclusive_lock_probe",
                "--nocapture",
            ])
            .env("TASKVEIL_PROFILE_LOCK_PROBE", &profile)
            .status()
            .unwrap();
        assert_eq!(probe.code(), Some(42));
    }

    #[cfg(unix)]
    #[test]
    fn replaced_session_lockfile_fails_closed_before_locking() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        std::fs::rename(
            profile.join(SESSION_LOCK_FILE_NAME),
            profile.join(".taskveil-session-token-set.lock.replaced"),
        )
        .unwrap();
        let replacement = File::create(profile.join(SESSION_LOCK_FILE_NAME)).unwrap();
        replacement
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .unwrap();
        assert!(matches!(
            coordinator.try_session(),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_hardlinked_lockfile_is_rejected() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let lockfile = profile.join(PROFILE_LOCK_FILE_NAME);
        File::create(&lockfile).unwrap();
        std::fs::hard_link(&lockfile, temp.path().join("lockfile-alias")).unwrap();
        assert!(matches!(
            ProfileCoordinator::for_profile(&profile),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_lockfile_is_rejected() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let target = temp.path().join("junction-target");
        std::fs::create_dir(&profile).unwrap();
        std::fs::create_dir(&target).unwrap();
        let lockfile = profile.join(PROFILE_LOCK_FILE_NAME);
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&lockfile)
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            ProfileCoordinator::for_profile(&profile),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_profile_acl_rejects_untrusted_effective_write_access() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let status = Command::new("icacls")
            .arg(&profile)
            .args(["/grant", "*S-1-5-32-546:(OI)(CI)M"])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            ProfileCoordinator::for_profile(&profile),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_profile_acl_rejects_inherit_only_untrusted_write_access() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        assert!(Command::new("icacls")
            .arg(&profile)
            .args(["/grant", "*S-1-5-32-546:(OI)(CI)(IO)M"])
            .status()
            .unwrap()
            .success());
        assert!(matches!(
            ProfileCoordinator::for_profile(&profile),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_lockfiles_reject_explicit_untrusted_write_access() {
        let temp = TempDir::new().unwrap();
        for (index, unsafe_name) in [PROFILE_LOCK_FILE_NAME, SESSION_LOCK_FILE_NAME]
            .into_iter()
            .enumerate()
        {
            let profile = temp.path().join(format!("profile-{index}"));
            std::fs::create_dir(&profile).unwrap();
            File::create(profile.join(PROFILE_LOCK_FILE_NAME)).unwrap();
            File::create(profile.join(SESSION_LOCK_FILE_NAME)).unwrap();
            assert!(Command::new("icacls")
                .arg(profile.join(unsafe_name))
                .args(["/grant", "*S-1-5-32-546:M"])
                .status()
                .unwrap()
                .success());
            assert!(matches!(
                ProfileCoordinator::for_profile(&profile),
                Err(ClientError::ProfileLockUnsupported)
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_database_rejects_explicit_untrusted_write_access() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let database = profile.join("taskveil.db");
        File::create(&database).unwrap();
        assert!(Command::new("icacls")
            .arg(&database)
            .args(["/grant", "*S-1-5-32-546:M"])
            .status()
            .unwrap()
            .success());
        assert!(matches!(
            coordinator.pin_database(&database),
            Err(ClientError::ProfileLockUnsupported)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_lockfile_cannot_be_replaced_while_coordinator_is_alive() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let _guard = coordinator.try_exclusive().unwrap();
        assert!(std::fs::rename(
            profile.join(PROFILE_LOCK_FILE_NAME),
            profile.join(".taskveil-profile.lock.replaced"),
        )
        .is_err());
        assert!(std::fs::rename(
            profile.join(SESSION_LOCK_FILE_NAME),
            profile.join(".taskveil-session-token-set.lock.replaced"),
        )
        .is_err());
    }

    #[test]
    fn child_profile_lock_actor() {
        let Ok(profile) = std::env::var("TASKVEIL_PROFILE_LOCK_CHILD") else {
            return;
        };
        let coordinator = ProfileCoordinator::for_profile(Path::new(&profile)).unwrap();
        let _guard = coordinator.try_exclusive().unwrap();
        if std::env::var_os("TASKVEIL_SPAWN_LOCK_GRANDCHILD").is_some() {
            let mut grandchild = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "profile_coordination::tests::grandchild_lock_inheritance_probe",
                    "--nocapture",
                ])
                .env("TASKVEIL_LOCK_GRANDCHILD", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            drop(_guard);
            println!("TASKVEIL_PROFILE_LOCK_GRANDCHILD_READY");
            std::io::stdout().flush().unwrap();
            grandchild.wait().unwrap();
            return;
        }
        println!("TASKVEIL_PROFILE_LOCK_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    #[test]
    fn grandchild_lock_inheritance_probe() {
        if std::env::var_os("TASKVEIL_LOCK_GRANDCHILD").is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    #[test]
    fn lock_handles_are_not_inherited_by_grandchildren() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_profile_lock_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_PROFILE_LOCK_CHILD", &profile)
            .env("TASKVEIL_SPAWN_LOCK_GRANDCHILD", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_PROFILE_LOCK_GRANDCHILD_READY") {
                break;
            }
        }
        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        coordinator.try_exclusive().unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn child_exclusive_lock_probe() {
        let Ok(profile) = std::env::var("TASKVEIL_PROFILE_LOCK_PROBE") else {
            return;
        };
        let coordinator = ProfileCoordinator::for_profile(Path::new(&profile)).unwrap();
        match coordinator.try_exclusive() {
            Ok(_) => {}
            Err(ClientError::ProfileBusy) => std::process::exit(42),
            Err(error) => panic!("exclusive profile lock probe failed: {error}"),
        }
    }

    #[test]
    fn child_session_lock_actor() {
        let Ok(profile) = std::env::var("TASKVEIL_SESSION_LOCK_CHILD") else {
            return;
        };
        let coordinator = ProfileCoordinator::for_profile(Path::new(&profile)).unwrap();
        let _guard = coordinator.try_session().unwrap();
        println!("TASKVEIL_SESSION_LOCK_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    #[test]
    fn child_session_lock_probe() {
        let Ok(profile) = std::env::var("TASKVEIL_SESSION_LOCK_PROBE") else {
            return;
        };
        let coordinator = ProfileCoordinator::for_profile(Path::new(&profile)).unwrap();
        match coordinator.try_session() {
            Ok(_) => {}
            Err(ClientError::Busy) => std::process::exit(42),
            Err(error) => panic!("session lock probe failed: {error}"),
        }
    }

    #[test]
    fn child_sync_lease_actor() {
        let Ok(profile) = std::env::var("TASKVEIL_SYNC_LEASE_CHILD") else {
            return;
        };
        let profile = Path::new(&profile);
        let coordinator = ProfileCoordinator::for_profile(profile).unwrap();
        let _profile_guard = coordinator.try_shared().unwrap();
        let db_path = profile.join("taskveil.db");
        let mut store = SqliteSyncStore::new(db_path, PROCESS_TEST_DB_KEY);
        store
            .acquire_sync_lease("child-sync-run", 1, 60_000)
            .unwrap();
        println!("TASKVEIL_SYNC_LEASE_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    #[test]
    fn real_process_sync_lease_allows_concurrent_local_mutation() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let db_path = profile.join("taskveil.db");
        let list = new_list(
            "Inbox".into(),
            "7fffffffffffffffffffffffffffffff".into(),
            100,
        )
        .unwrap();
        SqliteListRepository::new(open_encrypted(&db_path, &PROCESS_TEST_DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let task = new_task(
            list.id,
            None,
            "before".into(),
            "7fffffffffffffffffffffffffffffff".into(),
            100,
        )
        .unwrap();
        SqliteTaskRepository::new(open_encrypted(&db_path, &PROCESS_TEST_DB_KEY).unwrap())
            .insert(task.clone())
            .unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_sync_lease_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_SYNC_LEASE_CHILD", &profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_SYNC_LEASE_READY") {
                break;
            }
        }

        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let _profile_guard = coordinator.try_shared().unwrap();
        let tenant_id = taskveil_domain::Uuid::now_v7();
        let mutation = LocalMutationContext {
            device_id: "local-mutation".into(),
            keys: LocalSyncKeys {
                tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x41; 32])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
        };
        let updated = SqliteMutationService::new(db_path.clone(), PROCESS_TEST_DB_KEY)
            .update_task(
                UpdateTaskInput {
                    task_id: task.id,
                    title: "updated while sync waits".into(),
                    note: String::new(),
                    priority: 0,
                    due: None,
                    scheduled_at: None,
                    estimated_minutes: None,
                    now_ms: 101,
                },
                &mutation,
            )
            .unwrap();
        assert_eq!(updated.content.title, "updated while sync waits");
        assert_eq!(
            SqliteSyncStateRepository::new(open_encrypted(&db_path, &PROCESS_TEST_DB_KEY).unwrap())
                .list_outbox_heads(10)
                .unwrap()
                .len(),
            1
        );
        let mut competing_sync = SqliteSyncStore::new(db_path, PROCESS_TEST_DB_KEY);
        assert!(matches!(
            competing_sync.acquire_sync_lease("competing-sync-run", 1, 60_000),
            Err(taskveil_storage::StorageError::SyncLeaseBusy)
        ));

        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn session_sublock_excludes_another_process() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_session_lock_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_SESSION_LOCK_CHILD", &profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_SESSION_LOCK_READY") {
                break;
            }
        }
        let probe = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_session_lock_probe",
                "--nocapture",
            ])
            .env("TASKVEIL_SESSION_LOCK_PROBE", &profile)
            .status()
            .unwrap();
        assert_eq!(probe.code(), Some(42));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ProfileCoordinator::for_profile(&profile)
            .unwrap()
            .try_session()
            .is_ok());
    }

    #[test]
    fn dropping_one_shared_handle_does_not_release_the_other_process_lock() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        let first = coordinator.try_shared().unwrap();
        let second = coordinator.try_shared().unwrap();
        let probe = || {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "profile_coordination::tests::child_exclusive_lock_probe",
                    "--nocapture",
                ])
                .env("TASKVEIL_PROFILE_LOCK_PROBE", &profile)
                .status()
                .unwrap()
        };

        assert_eq!(probe().code(), Some(42));
        drop(first);
        assert_eq!(probe().code(), Some(42));
        drop(second);
        assert!(probe().success());
    }

    #[test]
    fn operating_system_lock_recovers_after_child_process_exit() {
        let temp = TempDir::new().unwrap();
        let profile = temp.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_coordination::tests::child_profile_lock_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_PROFILE_LOCK_CHILD", &profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_PROFILE_LOCK_READY") {
                break;
            }
        }

        let coordinator = ProfileCoordinator::for_profile(&profile).unwrap();
        assert!(matches!(
            coordinator.try_exclusive(),
            Err(ClientError::ProfileBusy)
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let _recovered = loop {
            match coordinator.try_exclusive() {
                Ok(guard) => break guard,
                Err(ClientError::ProfileBusy) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(error) => panic!("profile lock did not recover after process exit: {error}"),
            }
        };
    }
}
