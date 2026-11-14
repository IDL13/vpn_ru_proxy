use crate::Result;
use std::{
    env, io,
    path::PathBuf,
    process::{Child, Command},
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::register_host;

fn directory_variable(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(unix)]
fn home() -> Result<PathBuf> {
    directory_variable("HOME").ok_or_else(|| "Не найдена папка пользователя (HOME).".into())
}

#[cfg(target_os = "linux")]
fn linux_config_directory() -> Result<PathBuf> {
    match directory_variable("XDG_CONFIG_HOME") {
        Some(directory) => Ok(directory),
        None => Ok(home()?.join(".config")),
    }
}

#[cfg(windows)]
fn windows_app_data() -> Result<PathBuf> {
    directory_variable("LOCALAPPDATA")
        .ok_or_else(|| "Не найдена папка пользователя (LOCALAPPDATA).".into())
}

pub fn data_directory() -> Result<PathBuf> {
    if let Some(path) = env::var_os("VPN_RU_PROXY_DATA_DIR") {
        return Ok(path.into());
    }
    #[cfg(target_os = "macos")]
    let base = home()?.join("Library/Application Support");
    #[cfg(target_os = "linux")]
    let base = linux_config_directory()?;
    #[cfg(windows)]
    let base = windows_app_data()?;
    Ok(base.join("vpn-ru-proxy"))
}

pub fn manifest_directory(directory: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(directory) = directory {
        return Ok(std::path::absolute(directory)?.join("NativeMessagingHosts"));
    }
    #[cfg(target_os = "macos")]
    let base = home()?.join("Library/Application Support/Google/Chrome");
    #[cfg(target_os = "linux")]
    let base = if let Some(directory) = directory_variable("CHROME_USER_DATA_DIR") {
        directory
    } else if let Some(directory) = directory_variable("CHROME_CONFIG_HOME") {
        directory.join("google-chrome")
    } else {
        linux_config_directory()?.join("google-chrome")
    };
    #[cfg(windows)]
    let base = data_directory()?;
    Ok(std::path::absolute(base)?.join("NativeMessagingHosts"))
}

pub fn default_interface() -> &'static str {
    if cfg!(target_os = "macos") {
        "en0"
    } else {
        ""
    }
}

pub fn validate_interface(name: &str) -> Result<()> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("Укажи имя сетевого интерфейса в настройках расширения.".into());
    }
    #[cfg(unix)]
    let found = {
        let name = std::ffi::CString::new(name)?;
        // The name is NUL-terminated and stays valid for the call.
        unsafe { libc::if_nametoindex(name.as_ptr()) != 0 }
    };
    #[cfg(windows)]
    let found = windows::interface_exists(name);
    if !found {
        return Err(format!("Сетевой интерфейс «{name}» не найден. Выбери Wi-Fi или Ethernet в настройках расширения.").into());
    }
    Ok(())
}

pub fn sing_box_program() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SING_BOX_BIN") {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.is_file() {
            return Err(
                "SING_BOX_BIN должен содержать полный путь к исполняемому файлу sing-box.".into(),
            );
        }
        return Ok(path);
    }

    let name = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    let mut directories = Vec::new();
    #[cfg(target_os = "macos")]
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    #[cfg(target_os = "linux")]
    directories.extend([PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]);
    #[cfg(windows)]
    if let Ok(base) = windows_app_data() {
        directories.push(base.join("Microsoft/WinGet/Links"));
    }
    if let Some(path) = env::var_os("PATH") {
        directories.extend(env::split_paths(&path).filter(|directory| directory.is_absolute()));
    }
    if let Ok(executable) = env::current_exe() {
        directories.extend(executable.parent().map(PathBuf::from));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| format!("Не найден {name}. Установи sing-box, добавь его в PATH или укажи полный путь в SING_BOX_BIN.").into())
}

pub fn command(path: PathBuf) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        let mut command = Command::new(path);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    Command::new(path)
}

pub struct ProxyProcess {
    pub child: Child,
    #[cfg(windows)]
    _job: windows::Job,
}

impl ProxyProcess {
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;

            let parent = std::process::id() as libc::pid_t;
            // Only async-signal-safe operations run between fork and exec.
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    // The parent can exit before prctl installs the signal.
                    if libc::getppid() != parent {
                        return Err(io::ErrorKind::BrokenPipe.into());
                    }
                    Ok(())
                });
            }
        }

        #[cfg(windows)]
        let job = windows::Job::new()?;
        let child = command.spawn()?;
        #[cfg(windows)]
        {
            let mut child = child;
            if let Err(error) = job.assign(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Ok(Self { child, _job: job })
        }
        #[cfg(not(windows))]
        Ok(Self { child })
    }
}
