use crate::{Result, desktop_storage, platform, settings::Settings, site_rules::site_route_rules};
use serde_json::{Value, json};
use std::{
    fs, io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const PROXY_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 17892);

fn sing_box_config(settings: &Settings) -> Value {
    let rules = site_route_rules(&settings.sites, "without-vpn");
    let physical_dns = match settings.dns_server {
        Some(server) => {
            json!({"type": "udp", "tag": "physical-dns", "server": server, "bind_interface": settings.interface})
        }
        None => {
            json!({"type": "local", "tag": "physical-dns", "bind_interface": settings.interface})
        }
    };
    json!({
        "log": {"level": "info"},
        "dns": {"servers": [
            {"type": "local", "tag": "system-dns"},
            physical_dns
        ]},
        "inbounds": [{
            "type": "mixed",
            "listen": PROXY_ADDRESS.ip(),
            "listen_port": PROXY_ADDRESS.port(),
            "set_system_proxy": false
        }],
        "outbounds": [
            {"type": "direct", "tag": "system"},
            {"type": "direct", "tag": "without-vpn", "bind_interface": settings.interface, "domain_resolver": "physical-dns"}
        ],
        "route": {"default_domain_resolver": "system-dns", "rules": rules, "final": "system"}
    })
}

pub(crate) struct ProxyManager {
    directory: PathBuf,
    settings: Settings,
    child: Option<platform::ProxyProcess>,
    _lock: fs::File,
}

impl ProxyManager {
    pub(crate) fn load(directory: PathBuf) -> Result<Self> {
        desktop_storage::prepare(&directory)?;
        let lock = desktop_storage::write_options()
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

    pub(crate) fn running(&mut self) -> Result<bool> {
        if let Some(child) = &mut self.child
            && child.is_listening(PROXY_ADDRESS)?
        {
            return Ok(true);
        }
        self.stop()?;
        Ok(false)
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    fn sing_box(&self, operation: &str, config: &Path) -> Result<Command> {
        let mut command = platform::command(platform::sing_box_program()?);
        command
            .arg(operation)
            .arg("-D")
            .arg(&self.directory)
            .arg("-c")
            .arg(config);
        Ok(command)
    }

    fn check_config(&self, settings: &Settings) -> Result<PathBuf> {
        platform::validate_interface(&settings.interface)?;
        let candidate = self.directory.join("sing-box-candidate.json");
        desktop_storage::write_json(&candidate, &sing_box_config(settings))?;
        let output = self
            .sing_box("check", &candidate)?
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
        Ok(candidate)
    }

    fn launch(&mut self, candidate: &Path) -> Result<()> {
        let config = self.directory.join("sing-box.json");
        fs::rename(candidate, &config)?;
        // An extra listener can fail while old connections are in TIME_WAIT.
        let probe_timeout = Duration::from_millis(80);
        match TcpStream::connect_timeout(&SocketAddr::V4(PROXY_ADDRESS), probe_timeout) {
            Ok(_) => return Err(format!("Порт прокси {PROXY_ADDRESS} уже занят.").into()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => (),
            // Winsock may time out before reporting that a localhost port has no listener.
            // In that case sing-box's own bind determines whether the port is available.
            #[cfg(windows)]
            Err(error) if error.kind() == io::ErrorKind::TimedOut => (),
            Err(error) => {
                return Err(format!("Порт прокси {PROXY_ADDRESS} недоступен: {error}").into());
            }
        }
        let log_path = self.directory.join("sing-box.log");
        let log = desktop_storage::write_options()
            .create(true)
            .truncate(true)
            .open(&log_path)?;
        let mut command = self.sing_box("run", &config)?;
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        let child = self.child.insert(platform::ProxyProcess::spawn(command)?);
        let deadline = Instant::now() + Duration::from_secs(3);
        let ready = (|| -> io::Result<bool> {
            while Instant::now() < deadline && child.is_running()? {
                if child.is_listening(PROXY_ADDRESS)? {
                    return Ok(true);
                }
                thread::sleep(Duration::from_millis(40));
            }
            Ok(false)
        })();
        if matches!(ready, Ok(true)) {
            return Ok(());
        }
        self.stop()?;
        ready?;
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        Err(format!(
            "sing-box не открыл {PROXY_ADDRESS}. {}",
            log.lines().last().unwrap_or_default()
        )
        .into())
    }

    pub(crate) fn start(&mut self) -> Result<()> {
        if !self.running()? {
            let candidate = self.check_config(&self.settings)?;
            self.launch(&candidate)?;
        }
        Ok(())
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        if let Some(child) = &mut self.child {
            child.stop()?;
        }
        self.child = None;
        Ok(())
    }

    pub(crate) fn save(&mut self, settings: Settings) -> Result<()> {
        if settings == self.settings {
            return Ok(());
        }
        let candidate = if self.running()? {
            let candidate = self.check_config(&settings)?;
            self.stop()?;
            Some(candidate)
        } else {
            None
        };
        let result = (|| {
            if let Some(candidate) = &candidate {
                self.launch(candidate)?;
            }
            desktop_storage::write_json(&self.directory.join("settings.json"), &settings)
        })();
        if let Err(error) = result {
            if candidate.is_some()
                && let Err(rollback_error) = self.stop().and_then(|()| self.start())
            {
                return Err(format!(
                    "{error}. Восстановить предыдущий запуск не удалось: {rollback_error}"
                )
                .into());
            }
            return Err(error);
        }
        self.settings = settings;
        Ok(())
    }
}

impl Drop for ProxyManager {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::{Settings, sing_box_config};
    use serde_json::json;

    #[test]
    fn generates_domain_and_host_ip_rules_for_the_proxy() {
        let config = sing_box_config(&Settings {
            interface: "en0".into(),
            sites: vec![
                "example.org".into(),
                "192.0.2.1".into(),
                "2001:db8::1".into(),
            ],
            dns_server: None,
        });

        assert_eq!(config["inbounds"][0]["listen"], "127.0.0.1");
        assert_eq!(config["inbounds"][0]["listen_port"], 17892);
        assert_eq!(
            config["route"]["rules"],
            json!([
                {"domain": ["example.org"], "action": "route", "outbound": "without-vpn"},
                {"domain_suffix": [".example.org"], "action": "route", "outbound": "without-vpn"},
                {"ip_cidr": ["192.0.2.1/32", "2001:db8::1/128"], "action": "route", "outbound": "without-vpn"}
            ])
        );
    }
}
