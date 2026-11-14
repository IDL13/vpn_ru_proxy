mod amnezia;
mod native;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs, io,
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use url::{Host, Url};

const PROXY: &str = "127.0.0.1:17892";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct Settings {
    interface: String,
    sites: Vec<String>,
}

#[derive(Deserialize)]
struct Edit {
    interface: String,
    sites: String,
}

impl Edit {
    fn settings(self) -> Result<Settings> {
        let interface = self.interface.trim();
        if interface.is_empty() || interface.chars().any(char::is_control) {
            return Err("Укажи имя сетевого интерфейса.".into());
        }
        Ok(Settings {
            interface: interface.into(),
            sites: normalize_sites(
                self.sites
                    .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
                    .filter(|part| !part.is_empty()),
            )?,
        })
    }
}

fn normalize_sites<'a>(inputs: impl IntoIterator<Item = &'a str>) -> Result<Vec<String>> {
    let mut sites = inputs
        .into_iter()
        .map(|input| normalize(input).ok_or_else(|| format!("Некорректный сайт: {input}")))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    sites.sort_unstable();
    sites.dedup();
    Ok(sites)
}

fn normalize(input: &str) -> Option<String> {
    let input = input.trim();
    if let Ok(ip) = input.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    let url = if input.contains("://") {
        Url::parse(input).ok()?
    } else {
        Url::parse(&format!("https://{input}")).ok()?
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.host()? {
        Host::Domain(host) => {
            let host = host.strip_prefix("*.").unwrap_or(host);
            let host = host.strip_suffix('.').unwrap_or(host);
            if host.len() > 253
                || !host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return None;
            }
            let mut host = host;
            while let Some(parent) = host
                .strip_prefix("www.")
                .filter(|parent| parent.contains('.') && parent.parse::<IpAddr>().is_err())
            {
                host = parent;
            }
            Some(host.to_owned())
        }
        Host::Ipv4(ip) => Some(ip.to_string()),
        Host::Ipv6(ip) => Some(ip.to_string()),
    }
}

fn config(settings: &Settings) -> Value {
    let mut domains = Vec::new();
    let mut suffixes = Vec::new();
    let mut ips = Vec::new();
    for site in &settings.sites {
        if let Ok(ip) = site.parse::<IpAddr>() {
            ips.push(format!("{ip}/{}", if ip.is_ipv4() { 32 } else { 128 }));
        } else {
            domains.push(site.as_str());
            suffixes.push(format!(".{site}"));
        }
    }
    let mut rules = Vec::new();
    for (field, values) in [
        ("domain", domains),
        (
            "domain_suffix",
            suffixes.iter().map(String::as_str).collect(),
        ),
        ("ip_cidr", ips.iter().map(String::as_str).collect()),
    ] {
        if !values.is_empty() {
            rules.push(json!({ (field): values, "action": "route", "outbound": "without-vpn" }));
        }
    }
    json!({
        "log": {"level": "info"},
        "dns": {"servers": [
            {"type": "local", "tag": "system-dns"},
            {"type": "local", "tag": "physical-dns", "bind_interface": settings.interface}
        ]},
        "inbounds": [{"type": "mixed", "listen": "127.0.0.1", "listen_port": 17892, "set_system_proxy": false}],
        "outbounds": [
            {"type": "direct", "tag": "system"},
            {"type": "direct", "tag": "without-vpn", "bind_interface": settings.interface, "domain_resolver": "physical-dns"}
        ],
        "route": {"default_domain_resolver": "system-dns", "rules": rules, "final": "system"}
    })
}

struct Manager {
    directory: PathBuf,
    settings: Settings,
    child: Option<Child>,
    _lock: fs::File,
}

impl Settings {
    fn load(directory: &Path) -> Result<Self> {
        match fs::read(directory.join("settings.json")) {
            Ok(contents) => {
                let mut settings: Self = serde_json::from_slice(&contents)?;
                settings.sites = normalize_sites(settings.sites.iter().map(String::as_str))?;
                Ok(settings)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                interface: "en0".into(),
                sites: Vec::new(),
            }),
            Err(error) => Err(error.into()),
        }
    }
}

impl Manager {
    fn load(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory)?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("proxy.lock"))?;
        match lock.try_lock() {
            Ok(()) => (),
            Err(fs::TryLockError::WouldBlock) => {
                return Err(
                    "Прокси уже управляется из другого профиля Chrome. Выключи его в том профиле."
                        .into(),
                );
            }
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        let settings = Settings::load(&directory)?;
        Ok(Self {
            directory,
            settings,
            child: None,
            _lock: lock,
        })
    }

    fn running(&mut self) -> Result<bool> {
        if let Some(child) = &mut self.child {
            if child.try_wait()?.is_none() {
                return Ok(true);
            }
            self.child = None;
        }
        Ok(false)
    }

    fn sing_box(&self, operation: &str, config: &Path) -> Command {
        let program = env::var_os("SING_BOX_BIN").unwrap_or_else(|| {
            // Chrome launched from Finder may not inherit Homebrew's PATH.
            for path in ["/opt/homebrew/bin/sing-box", "/usr/local/bin/sing-box"] {
                if Path::new(path).is_file() {
                    return path.into();
                }
            }
            "sing-box".into()
        });
        let mut command = Command::new(program);
        command
            .arg(operation)
            .arg("-D")
            .arg(&self.directory)
            .arg("-c")
            .arg(config);
        command
    }

    fn prepare(&self, settings: &Settings) -> Result<()> {
        if settings.interface.is_empty() {
            return Err("Укажи сетевой интерфейс перед запуском.".into());
        }
        let candidate = self.directory.join("sing-box-candidate.json");
        write_json(&candidate, &config(settings))?;
        let output = self
            .sing_box("check", &candidate)
            .output()
            .map_err(|error| format!("Не удалось вызвать sing-box: {error}"))?;
        if !output.status.success() {
            let _ = fs::remove_file(&candidate);
            return Err(format!(
                "sing-box отклонил конфиг: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        fs::rename(candidate, self.directory.join("sing-box.json"))?;
        Ok(())
    }

    fn launch(&mut self) -> Result<()> {
        drop(
            TcpListener::bind(PROXY)
                .map_err(|error| format!("Порт прокси {PROXY} недоступен: {error}"))?,
        );
        let log_path = self.directory.join("sing-box.log");
        let log = fs::File::create(&log_path)?;
        self.child = Some(
            self.sing_box("run", &self.directory.join("sing-box.json"))
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let address: SocketAddr = PROXY.parse()?;
        while self.running()? && Instant::now() < deadline {
            if TcpStream::connect_timeout(&address, Duration::from_millis(80)).is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(40));
        }
        self.stop()?;
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        Err(format!(
            "sing-box не открыл {PROXY}. {}",
            log.lines().last().unwrap_or_default()
        )
        .into())
    }

    fn start(&mut self) -> Result<()> {
        if !self.running()? {
            self.prepare(&self.settings)?;
            self.launch()?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if self.running()? {
            let child = self.child.as_mut().expect("running child");
            if let Err(error) = child.kill() {
                if child.try_wait()?.is_none() {
                    return Err(error.into());
                }
            }
            child.wait()?;
            self.child = None;
        }
        Ok(())
    }

    fn save(&mut self, settings: Settings) -> Result<()> {
        if settings == self.settings {
            return Ok(());
        }
        let was_running = self.running()?;
        if was_running {
            self.prepare(&settings)?;
            self.stop()?;
        }
        let result = (|| {
            if was_running {
                self.launch()?;
            }
            write_json(&self.directory.join("settings.json"), &settings)
        })();
        if let Err(error) = result {
            if was_running {
                if let Err(rollback_error) = self.stop().and_then(|_| self.start()) {
                    return Err(format!(
                        "{error}. Восстановить предыдущий запуск не удалось: {rollback_error}"
                    )
                    .into());
                }
            }
            return Err(error);
        }
        self.settings = settings;
        Ok(())
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let next = path.with_extension("next");
    fs::write(&next, serde_json::to_vec_pretty(value)?)?;
    fs::rename(next, path)?;
    Ok(())
}

fn data_directory() -> Result<PathBuf> {
    if let Some(path) = env::var_os("VPN_RU_PROXY_DATA_DIR") {
        return Ok(path.into());
    }
    let home = env::var_os("HOME").ok_or("Не найдена папка пользователя")?;
    Ok(PathBuf::from(home).join("Library/Application Support/vpn-ru-proxy"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let argument = arguments.next().unwrap_or_default();
    if argument == "--install-chrome-host" {
        let extension_id = arguments.next().unwrap_or_default();
        return native::install(
            &extension_id.to_string_lossy(),
            arguments.next().map(PathBuf::from),
        );
    }
    if argument
        .to_string_lossy()
        .starts_with("chrome-extension://")
    {
        return native::run().await;
    }
    Err("Открой VPN RU Proxy в Chrome. Для установки помощника используй --install-chrome-host ID_РАСШИРЕНИЯ.".into())
}

async fn shutdown() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}
