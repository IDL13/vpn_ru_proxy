use std::{
    ffi::OsStr,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    process::Child,
    ptr::null,
};
use windows_sys::Win32::{
    NetworkManagement::{IpHelper::ConvertInterfaceAliasToLuid, Ndis::NET_LUID_LH},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ},
    },
};

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub fn register_host(name: &str, manifest: &Path) -> io::Result<()> {
    let key = wide(OsStr::new(&format!(
        r"Software\Google\Chrome\NativeMessagingHosts\{name}"
    )));
    let value = wide(manifest.as_os_str());
    // A null value name writes the default value; both strings are NUL-terminated.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            null(),
            REG_SZ,
            value.as_ptr().cast(),
            std::mem::size_of_val(value.as_slice()) as u32,
        )
    };
    match status {
        0 => Ok(()),
        error => Err(io::Error::from_raw_os_error(error as i32)),
    }
}

pub fn interface_exists(name: &str) -> bool {
    let alias = wide(OsStr::new(name));
    let mut luid = NET_LUID_LH::default();
    // The input is NUL-terminated and luid has writable storage for the result.
    unsafe { ConvertInterfaceAliasToLuid(alias.as_ptr(), &mut luid) == 0 }
}

pub struct Job(OwnedHandle);

impl Job {
    pub fn new() -> io::Result<Self> {
        // Null arguments create an unnamed job with a non-inheritable handle.
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // Take ownership of the fresh handle exactly once.
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // The pointer and length describe the extended limit information structure.
        let success = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // Both handle owners remain alive for the call.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
