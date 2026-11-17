use std::{
    ffi::OsStr,
    io,
    net::SocketAddrV4,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    process::Child,
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    NetworkManagement::{
        IpHelper::{
            ConvertInterfaceAliasToLuid, GetExtendedTcpTable, MIB_TCPROW_OWNER_PID,
            MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
        },
        Ndis::NET_LUID_LH,
    },
    Networking::WinSock::AF_INET,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW},
    },
};

pub(super) fn owns_tcp_listener(pid: u32, address: SocketAddrV4) -> io::Result<bool> {
    let mut table = vec![0_u32; 1024];
    let local_address = u32::from_ne_bytes(address.ip().octets());
    let local_port = u32::from(address.port().to_be());
    let row_size = size_of::<MIB_TCPROW_OWNER_PID>() / size_of::<u32>();
    let row_offset = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table) / size_of::<u32>();
    loop {
        let mut bytes = u32::try_from(std::mem::size_of_val(table.as_slice()))
            .map_err(|_| io::ErrorKind::InvalidData)?;
        // SAFETY: The buffer is writable, suitably aligned for the table's DWORDs,
        // and bytes contains its capacity until the call returns.
        let status = unsafe {
            GetExtendedTcpTable(
                table.as_mut_ptr().cast(),
                &raw mut bytes,
                0,
                u32::from(AF_INET),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            table.resize((bytes as usize).div_ceil(size_of::<u32>()), 0);
            continue;
        }
        if status != 0 {
            return Err(io::Error::from_raw_os_error(i32::from_ne_bytes(
                status.to_ne_bytes(),
            )));
        }
        let count = table[0] as usize;
        let rows = table.get(row_offset..).ok_or(io::ErrorKind::InvalidData)?;
        if count > rows.len() / row_size {
            return Err(io::ErrorKind::InvalidData.into());
        }
        // MIB_TCPROW_OWNER_PID stores state, local address and port, remote
        // address and port, and then the owning PID, all as DWORDs.
        return Ok(rows
            .chunks_exact(row_size)
            .take(count)
            .any(|row| row[1] == local_address && row[2] & 0xffff == local_port && row[5] == pid));
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub fn register_host(name: &str, manifest: &Path) -> io::Result<()> {
    let key = wide(OsStr::new(&format!(
        r"Software\Google\Chrome\NativeMessagingHosts\{name}"
    )));
    let value = wide(manifest.as_os_str());
    let value_size = u32::try_from(std::mem::size_of_val(value.as_slice()))
        .map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: Both strings are NUL-terminated and value_size is the byte size of
    // the value buffer. A null value name selects the default registry value.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            null(),
            REG_SZ,
            value.as_ptr().cast(),
            value_size,
        )
    };
    match status {
        0 => Ok(()),
        error => Err(io::Error::from_raw_os_error(i32::from_ne_bytes(
            error.to_ne_bytes(),
        ))),
    }
}

pub fn interface_exists(name: &str) -> bool {
    let alias = wide(OsStr::new(name));
    let mut luid = NET_LUID_LH::default();
    // SAFETY: The alias is NUL-terminated and luid has writable storage for the result.
    unsafe { ConvertInterfaceAliasToLuid(alias.as_ptr(), &raw mut luid) == 0 }
}

pub struct Job(OwnedHandle);

impl Job {
    pub fn new() -> io::Result<Self> {
        // SAFETY: The API accepts null arguments for an unnamed, non-inheritable job.
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: The successful call returned a fresh handle, owned here exactly once.
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let limits_size = u32::try_from(std::mem::size_of_val(&limits))
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: The handle is live and the pointer and byte length describe
        // the requested extended limit information structure.
        let success = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                limits_size,
            )
        };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: The job and child handles are valid for the duration of the call.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
