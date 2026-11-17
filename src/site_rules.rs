use serde_json::{Value, json};
use std::net::IpAddr;

pub(crate) fn site_route_rules(sites: &[String], outbound: &str) -> Vec<Value> {
    let mut domains = Vec::new();
    let mut suffixes = Vec::new();
    let mut ips = Vec::new();
    for site in sites {
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
            rules.push(json!({ (field): values, "action": "route", "outbound": outbound }));
        }
    }
    rules
}
