use crate::{
    Result, platform,
    settings::normalize_sites,
    site_import::{AmneziaEntry, import_amnezia_sites},
    site_rules::site_route_rules,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    net::IpAddr,
    path::Path,
    process::Stdio,
};

const VPN_TAG: &str = "vpn";
const DIRECT_TAG: &str = "direct";
const VPN_DNS_TAG: &str = "vpn-dns";

#[derive(Deserialize)]
#[serde(untagged)]
enum SiteList {
    Amnezia(Vec<AmneziaEntry>),
    Settings { sites: Vec<String> },
}

pub(crate) fn export(mut arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let first = arguments.next();
    let include_ips = first.as_deref().is_some_and(|arg| arg == "--include-ips");
    let sites = if include_ips { arguments.next() } else { first };
    let (Some(sites), Some(base), Some(dns), Some(output), None) = (
        sites,
        arguments.next(),
        arguments.next(),
        arguments.next(),
        arguments.next(),
    ) else {
        return Err(concat!(
            "Использование: vpn-ru-proxy export-mobile [--include-ips] ",
            "СПИСОК.json БАЗОВЫЙ_ПРОФИЛЬ.json IP_DNS НОВЫЙ_ФАЙЛ.json"
        )
        .into());
    };
    let vpn_dns = dns
        .to_str()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .ok_or("Укажи IP-адрес DNS-сервера, доступного через VPN.")?;
    let sites = mobile_sites(serde_json::from_slice(&fs::read(sites)?)?, include_ips)?;
    let base = serde_json::from_slice(&fs::read(base)?)?;
    let endpoint = wireguard_endpoint(&base)?;
    let profile = serde_json::to_vec_pretty(&mobile_config(&sites, endpoint, vpn_dns))?;
    check_config(&profile)?;
    let output = Path::new(&output);
    write_profile(output, &profile)?;
    println!("Мобильный профиль сохранён в {}", output.display());
    Ok(())
}

fn mobile_sites(list: SiteList, include_ips: bool) -> Result<Vec<String>> {
    let mut sites: Vec<String> = match list {
        SiteList::Amnezia(entries) => import_amnezia_sites(entries)?.into_iter().collect(),
        SiteList::Settings { sites } => normalize_sites(sites.iter().map(String::as_str))?,
    };
    if !include_ips {
        sites.retain(|site| site.parse::<IpAddr>().is_err());
    }
    Ok(sites)
}

fn wireguard_endpoint(base: &Value) -> Result<&Value> {
    let endpoint = base["endpoints"]
        .as_array()
        .and_then(|endpoints| endpoints.iter().find(|endpoint| endpoint["tag"] == VPN_TAG))
        .filter(|endpoint| endpoint["type"] == "wireguard")
        .ok_or("В базовом профиле нужен endpoint типа wireguard с тегом vpn.")?;
    if endpoint["system"] == true
        || endpoint.get("bind_interface").is_some()
        || endpoint.get("detour").is_some()
    {
        return Err(
            "WireGuard endpoint должен работать без system, bind_interface и detour.".into(),
        );
    }
    let Some([peer]) = endpoint["peers"].as_array().map(Vec::as_slice) else {
        return Err("Для мобильного профиля укажи одного WireGuard peer.".into());
    };
    peer["address"]
        .as_str()
        .and_then(|address| address.parse::<IpAddr>().ok())
        .ok_or("Укажи IP-адрес WireGuard-сервера в peers[0].address.")?;
    Ok(endpoint)
}

fn mobile_config(sites: &[String], endpoint: &Value, vpn_dns: IpAddr) -> Value {
    let mut rules = vec![
        json!({"action": "sniff"}),
        json!({"protocol": "dns", "action": "hijack-dns"}),
    ];
    rules.extend(site_route_rules(sites, DIRECT_TAG));

    json!({
        "log": {"level": "warn"},
        "inbounds": [{
            "type": "tun",
            "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
            "auto_route": true
        }],
        "outbounds": [{"type": "direct", "tag": DIRECT_TAG}],
        "endpoints": [endpoint],
        "dns": {
            "servers": [{"type": "udp", "tag": VPN_DNS_TAG, "server": vpn_dns, "detour": VPN_TAG}],
            "final": VPN_DNS_TAG
        },
        "route": {"rules": rules, "final": VPN_TAG, "default_domain_resolver": VPN_DNS_TAG}
    })
}

fn check_config(profile: &[u8]) -> Result<()> {
    let mut checker = platform::command(platform::sing_box_program()?)
        .args(["check", "-c", "stdin", "--disable-color"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = checker
        .stdin
        .take()
        .ok_or("Не найден stdin процесса sing-box.")?;
    let written = input.write_all(profile);
    drop(input);
    let output = checker.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "sing-box отклонил мобильный профиль: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    written?;
    Ok(())
}

fn write_profile(path: &Path, profile: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if let Err(error) = file.write_all(profile) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{mobile_config, mobile_sites, wireguard_endpoint};
    use serde_json::json;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn includes_ip_rules_only_when_requested_for_either_import_format() -> crate::Result<()> {
        let sources = [
            json!([{"hostname": "*.example.org", "ip": "192.0.2.1", "ips": ["2001:db8::1"]}]),
            json!({"sites": ["https://www.example.org", "192.0.2.1", "2001:db8::1"]}),
        ];
        let endpoint = json!({"type": "wireguard", "tag": "vpn"});
        let dns = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53));
        for source in sources {
            let domains = mobile_sites(serde_json::from_value(source.clone())?, false)?;
            assert_eq!(domains, ["example.org"]);
            let config = mobile_config(&domains, &endpoint, dns);
            assert_eq!(config["route"]["rules"].as_array().map(Vec::len), Some(4));
            assert_eq!(
                config["route"]["rules"][2]["domain"],
                json!(["example.org"])
            );
            assert_eq!(
                config["route"]["rules"][3]["domain_suffix"],
                json!([".example.org"])
            );

            let sites = mobile_sites(serde_json::from_value(source)?, true)?;
            let config = mobile_config(&sites, &endpoint, dns);
            assert_eq!(
                config["route"]["rules"][4]["ip_cidr"],
                json!(["192.0.2.1/32", "2001:db8::1/128"])
            );
        }
        Ok(())
    }

    #[test]
    fn routes_only_listed_destinations_directly_and_keeps_dns_in_vpn() {
        let endpoint = json!({"type": "wireguard", "tag": "vpn"});
        let profile = mobile_config(
            &[
                "example.org".into(),
                "192.0.2.1".into(),
                "2001:db8::1".into(),
            ],
            &endpoint,
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53)),
        );
        assert_eq!(profile["inbounds"][0]["type"], "tun");
        assert_eq!(profile["route"]["rules"][0]["action"], "sniff");
        assert_eq!(profile["route"]["rules"][1]["action"], "hijack-dns");
        assert_eq!(
            profile["route"]["rules"][2]["domain"],
            json!(["example.org"])
        );
        assert_eq!(
            profile["route"]["rules"][3]["domain_suffix"],
            json!([".example.org"])
        );
        assert_eq!(
            profile["route"]["rules"][4]["ip_cidr"],
            json!(["192.0.2.1/32", "2001:db8::1/128"])
        );
        for index in 2..5 {
            assert_eq!(profile["route"]["rules"][index]["outbound"], "direct");
        }
        assert_eq!(profile["route"]["final"], "vpn");
        assert_eq!(profile["dns"]["servers"][0]["detour"], "vpn");
    }

    #[test]
    fn requires_ip_for_the_wireguard_peer_to_avoid_dns_bootstrap_loop() {
        let mut base = json!({
            "endpoints": [{"type": "wireguard", "tag": "vpn", "peers": [{"address": "vpn.example.org"}]}]
        });
        assert!(wireguard_endpoint(&base).is_err());
        base["endpoints"][0]["peers"][0]["address"] = json!("2001:db8::1");
        assert!(wireguard_endpoint(&base).is_ok());
    }
}
