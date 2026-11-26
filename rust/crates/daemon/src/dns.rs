use std::net::{IpAddr, Ipv6Addr};

pub fn parse_default_dns(dump: &str) -> Vec<String> {
    let Some(default_net) = dump.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Active default network: ")?
            .parse::<u32>()
            .ok()
    }) else {
        return Vec::new();
    };
    let Some(addresses) = dump.lines().find_map(|line| {
        let (net, info) = line
            .trim_start()
            .strip_prefix("NetworkAgentInfo{network{")?
            .split_once('}')?;
        if net.parse::<u32>().ok()? != default_net {
            return None;
        }
        let (_, list) = info.split_once("DnsAddresses: [")?;
        Some(list.split_once(']')?.0)
    }) else {
        return Vec::new();
    };

    let mut servers = Vec::new();
    for entry in addresses.split(',') {
        let address = entry.trim();
        let address = address.strip_prefix('/').unwrap_or(address);
        if valid_address(address) && !servers.iter().any(|server| server == address) {
            servers.push(address.to_owned());
            if servers.len() == 3 {
                break;
            }
        }
    }
    servers
}

fn valid_address(address: &str) -> bool {
    if let Some((ip, zone)) = address.split_once('%') {
        ip.parse::<Ipv6Addr>().is_ok()
            && !zone.is_empty()
            && zone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    } else {
        address.parse::<IpAddr>().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_default_dns;

    #[test]
    fn selects_dns_from_the_active_network() {
        let dump = "Active default network: 103

Current Networks:
  NetworkAgentInfo{network{101} lp{{DnsAddresses: [ /192.0.2.53 ]}} Transports: CELLULAR}
  NetworkAgentInfo{network{103} lp{{DnsAddresses: [ /2001:db8::53,/198.51.100.53 ]}} Transports: WIFI}
";
        assert_eq!(parse_default_dns(dump), ["2001:db8::53", "198.51.100.53"]);
        assert_eq!(
            parse_default_dns(&dump.replace("default network: 103", "default network: 101")),
            ["192.0.2.53"]
        );
        assert!(
            parse_default_dns(&dump.replace("default network: 103", "default network: none"))
                .is_empty()
        );
    }

    #[test]
    fn keeps_three_unique_resolver_addresses() {
        let dump = "Active default network: 8
  NetworkAgentInfo{network{8} lp{{DnsAddresses: [ /fe80::53%wlan0,/invalid,/fe80::53%wlan0,/192.0.2.53,/192.0.2.54,/192.0.2.55 ]}}}
";
        assert_eq!(
            parse_default_dns(dump),
            ["fe80::53%wlan0", "192.0.2.53", "192.0.2.54"]
        );
    }
}
