use crate::{normalize, Result};
use serde::Deserialize;
use std::{collections::BTreeSet, net::IpAddr};

#[derive(Deserialize)]
pub struct Entry {
    hostname: String,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    ips: Vec<String>,
}

pub fn sites(entries: Vec<Entry>) -> Result<BTreeSet<String>> {
    let mut sites = BTreeSet::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let hostname = entry.hostname.trim();
        if !hostname.is_empty() {
            let site = normalize(hostname)
                .ok_or_else(|| format!("Некорректный hostname в записи №{}.", index + 1))?;
            sites.insert(site);
        }
        for value in std::iter::once(entry.ip).chain(entry.ips) {
            let value = value.trim();
            if !value.is_empty() {
                let ip: IpAddr = value
                    .parse()
                    .map_err(|_| format!("Некорректный IP в записи №{}.", index + 1))?;
                sites.insert(ip.to_string());
            }
        }
    }
    if sites.is_empty() {
        return Err("В JSON нет доменов или IP-адресов для импорта.".into());
    }
    Ok(sites)
}
