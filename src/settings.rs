use crate::{Result, platform};
use serde::{Deserialize, Serialize};
use std::{fs, io, net::IpAddr, path::Path};
use url::{Host, Url};

#[derive(Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) interface: String,
    pub(crate) sites: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dns_server: Option<IpAddr>,
}

#[derive(Deserialize)]
pub(crate) struct SettingsEdit {
    interface: String,
    sites: String,
    #[serde(default)]
    dns_server: String,
}

impl SettingsEdit {
    pub(crate) fn into_settings(self) -> Result<Settings> {
        let interface = self.interface.trim();
        if interface.is_empty() || interface.chars().any(char::is_control) {
            return Err("Укажи имя сетевого интерфейса.".into());
        }
        let dns_server = self.dns_server.trim();
        let dns_server = if dns_server.is_empty() {
            None
        } else {
            Some(
                dns_server
                    .parse()
                    .map_err(|_| "Укажи IP-адрес DNS-сервера.")?,
            )
        };
        Ok(Settings {
            interface: interface.into(),
            sites: normalize_sites(
                self.sites
                    .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
                    .filter(|part| !part.is_empty()),
            )?,
            dns_server,
        })
    }
}

pub(crate) fn normalize_sites<'a>(
    inputs: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<String>> {
    let mut sites = inputs
        .into_iter()
        .map(|input| normalize_site(input).ok_or_else(|| format!("Некорректный сайт: {input}")))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    sites.sort_unstable();
    sites.dedup();
    Ok(sites)
}

pub(crate) fn normalize_site(input: &str) -> Option<String> {
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

impl Settings {
    pub(crate) fn load(directory: &Path) -> Result<Self> {
        match fs::read(directory.join("settings.json")) {
            Ok(contents) => {
                let mut settings: Self = serde_json::from_slice(&contents)?;
                settings.sites = normalize_sites(settings.sites.iter().map(String::as_str))?;
                Ok(settings)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                interface: platform::default_interface().into(),
                sites: Vec::new(),
                dns_server: None,
            }),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsEdit;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn normalizes_and_deduplicates_sites_in_settings_edits() -> crate::Result<()> {
        let settings = SettingsEdit {
            interface: " en0 ".into(),
            sites: "https://www.Example.org/path,*.example.org.; 2001:0db8::1 192.0.2.1".into(),
            dns_server: " 192.0.2.53 ".into(),
        }
        .into_settings()?;

        assert_eq!(settings.interface, "en0");
        assert_eq!(settings.sites, ["192.0.2.1", "2001:db8::1", "example.org"]);
        assert_eq!(
            settings.dns_server,
            Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53)))
        );
        Ok(())
    }
}
