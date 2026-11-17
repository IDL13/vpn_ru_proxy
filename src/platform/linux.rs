use std::{collections::HashSet, fs, io, net::SocketAddrV4, path::Path};

pub(super) fn owns_tcp_listener(pid: u32, address: SocketAddrV4) -> io::Result<bool> {
    let process = format!("/proc/{pid}");
    let target = format!(
        "{:08X}:{:04X}",
        u32::from_ne_bytes(address.ip().octets()),
        address.port()
    );
    // The table covers the child's network namespace. Its file descriptors identify ownership.
    let table = match fs::read_to_string(Path::new(&process).join("net/tcp")) {
        Ok(table) => table,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let inodes: HashSet<_> = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let local_address = fields.nth(1)?;
            let state = fields.nth(1)?;
            if local_address != target || state != "0A" {
                return None;
            }
            fields.nth(5)
        })
        .collect();
    if inodes.is_empty() {
        return Ok(false);
    }

    let descriptors = match fs::read_dir(Path::new(&process).join("fd")) {
        Ok(descriptors) => descriptors,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for descriptor in descriptors {
        let link = match fs::read_link(descriptor?.path()) {
            Ok(link) => link,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if link
            .to_str()
            .and_then(|link| link.strip_prefix("socket:[")?.strip_suffix(']'))
            .is_some_and(|inode| inodes.contains(inode))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
