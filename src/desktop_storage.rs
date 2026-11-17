use crate::Result;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

pub(crate) fn prepare(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let data = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(directory)?;
        restrict(&data, 0o700)?;

        let mut read = OpenOptions::new();
        read.read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        for name in [
            "proxy.lock",
            "settings.json",
            "settings.next",
            "sing-box.json",
            "sing-box-candidate.json",
            "sing-box-candidate.next",
            "sing-box.log",
        ] {
            match read.open(directory.join(name)) {
                Ok(file) if file.metadata()?.is_file() => restrict(&file, 0o600)?,
                Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    fs::create_dir_all(directory)
}

#[cfg(unix)]
fn restrict(file: &fs::File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if file.metadata()?.permissions().mode() & 0o7777 != mode {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub(crate) fn write_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let contents = serde_json::to_vec_pretty(value)?;
    let next = path.with_extension("next");
    let mut file = write_options().create(true).truncate(true).open(&next)?;
    #[cfg(unix)]
    restrict(&file, 0o600)?;
    file.write_all(&contents)?;
    drop(file);
    fs::rename(next, path)?;
    Ok(())
}
