use protocol::PortForward;
use std::collections::HashSet;
use std::io;
use std::path::{Component, Path};

pub const MAX_PORT_FORWARDS: usize = 32;

pub fn validate_distro_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "distro name must be 1..=64 bytes",
        ));
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "distro name must start with ASCII alphanumeric",
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "distro name contains disallowed characters",
        ));
    }
    Ok(())
}

pub fn validate_image_path(path: &str) -> io::Result<()> {
    if path.is_empty() || path.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid image path",
        ));
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "image path must be absolute",
        ));
    }
    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "image path must not contain '..'",
            ));
        }
    }
    Ok(())
}

pub fn validate_username(name: &str) -> io::Result<()> {
    let bytes = name.as_bytes();
    let valid = (1..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "username must be 1..=32 of a-z, 0-9, '_' or '-', starting with a letter or '_'",
        ))
    }
}

pub fn validate_os_label(label: &str) -> io::Result<()> {
    if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "OS label must be 1..=64 printable characters",
        ));
    }
    Ok(())
}

pub fn validate_ssh_port(port: u16) -> io::Result<()> {
    if port == 0 || port >= 1024 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SSH port must be 0 or 1024..=65535",
        ))
    }
}

pub fn validate_port_forwards(forwards: &[PortForward], ssh_port: u16) -> io::Result<()> {
    if forwards.len() > MAX_PORT_FORWARDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("at most {MAX_PORT_FORWARDS} port forwards are allowed"),
        ));
    }
    let mut seen_host = HashSet::with_capacity(forwards.len());
    for fwd in forwards {
        if fwd.host_port < 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "forwarded host port must be 1024..=65535",
            ));
        }
        if fwd.guest_port == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "forwarded guest port must be 1..=65535",
            ));
        }
        if ssh_port != 0 && fwd.host_port == ssh_port {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("host port {} conflicts with SSH port", fwd.host_port),
            ));
        }
        if !seen_host.insert(fwd.host_port) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate forwarded host port {}", fwd.host_port),
            ));
        }
    }
    Ok(())
}

pub fn parse_port_forwards(text: &str) -> io::Result<Vec<PortForward>> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for token in trimmed.split([',', '\n', ';']) {
        let item = token.trim();
        if item.is_empty() {
            continue;
        }
        let (host_s, guest_s) = match item.split_once(':') {
            Some((h, g)) => (h.trim(), g.trim()),
            None => (item, item),
        };
        let host_port = host_s
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid host port: {item}")))?;
        let guest_port = guest_s
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid guest port: {item}")))?;
        out.push(PortForward { host_port, guest_port });
    }
    Ok(out)
}

pub fn format_port_forwards(forwards: &[PortForward]) -> String {
    forwards
        .iter()
        .map(|f| format!("{}:{}", f.host_port, f.guest_port))
        .collect::<Vec<_>>()
        .join(",")
}

pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub home: String,
    pub shell: String,
}

pub fn parse_passwd_line(line: &str) -> Option<PasswdEntry> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 7 {
        return None;
    }
    Some(PasswdEntry {
        name: fields[0].to_string(),
        uid: fields[2].parse().ok()?,
        home: fields[5].to_string(),
        shell: fields[6].to_string(),
    })
}

pub fn enable_pacman_option(conf: &str, option: &str) -> Option<String> {
    if conf.lines().any(|l| l.trim() == option) {
        return None;
    }
    let commented = format!("#{option}");
    let mut lines: Vec<String> = conf.lines().map(str::to_string).collect();
    if let Some(line) = lines.iter_mut().find(|l| l.trim() == commented) {
        *line = option.to_string();
    } else {
        let at = lines.iter().position(|l| l.trim() == "[options]")?;
        lines.insert(at + 1, option.to_string());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Some(out)
}

pub const ANDROID_MEDIA_GID: u32 = 1023;

pub fn member_groups(group_text: &str, user: &str) -> Vec<u32> {
    group_text
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let gid = fields.get(2)?.parse::<u32>().ok()?;
            fields.get(3)?.split(',').any(|m| m.trim() == user).then_some(gid)
        })
        .collect()
}

pub fn validate_session_env(env: &[String]) -> io::Result<()> {
    let valid = env.iter().all(|entry| {
        entry.split_once('=').is_some_and(|(key, _)| {
            !key.is_empty()
                && key.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                && key.starts_with("BAREBOOT_")
        })
    });
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tab environment entries must look like BAREBOOT_NAME=value",
        ))
    }
}

pub fn parse_allowed_uids(content: &str) -> Vec<u32> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.split('#').next().unwrap_or("").trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<u32>().ok()
            }
        })
        .collect()
}

pub fn valid_timezone(timezone: &str) -> bool {
    (1..=64).contains(&timezone.len())
        && !timezone.starts_with('/')
        && timezone.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && timezone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
}

pub fn format_resolv_conf(servers: &[String]) -> String {
    let mut out = String::new();
    for s in servers {
        let trimmed = s.trim();
        if !trimmed.is_empty() && !trimmed.contains('\n') && !trimmed.contains('\r') {
            out.push_str("nameserver ");
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    if out.is_empty() {
        out.push_str("nameserver 1.1.1.1\nnameserver 8.8.8.8\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_name_validation() {
        assert!(validate_distro_name("debian").is_ok());
        assert!(validate_distro_name("debian-13_arm64.1").is_ok());
        assert!(validate_distro_name("").is_err());
        assert!(validate_distro_name("../etc").is_err());
        assert!(validate_distro_name("-bad").is_err());
        assert!(validate_distro_name("a/b").is_err());
    }

    #[test]
    fn image_path_validation() {
        assert!(validate_image_path("/data/adb/bareboot/debian.img").is_ok());
        assert!(validate_image_path("relative.img").is_err());
        assert!(validate_image_path("/data/../etc/passwd").is_err());
        assert!(validate_image_path("/data/a\0b").is_err());
    }

    #[test]
    fn ssh_port_validation() {
        assert!(validate_ssh_port(0).is_ok());
        assert!(validate_ssh_port(1024).is_ok());
        assert!(validate_ssh_port(2222).is_ok());
        assert!(validate_ssh_port(65535).is_ok());
        assert!(validate_ssh_port(1).is_err());
        assert!(validate_ssh_port(22).is_err());
        assert!(validate_ssh_port(1023).is_err());
    }

    #[test]
    fn port_forwards_validation_and_parsing() {
        let parsed = parse_port_forwards("8080:80,3000").unwrap();
        assert_eq!(
            parsed,
            vec![
                PortForward { host_port: 8080, guest_port: 80 },
                PortForward { host_port: 3000, guest_port: 3000 },
            ]
        );
        assert_eq!(format_port_forwards(&parsed), "8080:80,3000:3000");
        assert!(parse_port_forwards("-").unwrap().is_empty());
        assert!(validate_port_forwards(&parsed, 2222).is_ok());
        assert!(validate_port_forwards(&parsed, 8080).is_err());
        assert!(validate_port_forwards(&[PortForward { host_port: 80, guest_port: 80 }], 0).is_err());
        assert!(validate_port_forwards(&[PortForward { host_port: 8080, guest_port: 0 }], 0).is_err());
        assert!(validate_port_forwards(
            &[
                PortForward { host_port: 8080, guest_port: 80 },
                PortForward { host_port: 8080, guest_port: 81 },
            ],
            0
        ).is_err());
    }

    #[test]
    fn username_validation() {
        assert!(validate_username("debian").is_ok());
        assert!(validate_username("_build-1").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("Root").is_err());
        assert!(validate_username("1user").is_err());
        assert!(validate_username("a:b").is_err());
        assert!(validate_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn passwd_line_parsing() {
        let entry = parse_passwd_line("alpine:x:1000:1000::/home/alpine:/bin/ash").unwrap();
        assert_eq!(entry.name, "alpine");
        assert_eq!(entry.uid, 1000);
        assert_eq!(entry.home, "/home/alpine");
        assert_eq!(entry.shell, "/bin/ash");
        assert!(parse_passwd_line("broken:x:1000").is_none());
        assert!(parse_passwd_line("bad:x:uid:0::/:/bin/sh").is_none());
    }

    #[test]
    fn pacman_option_enabling() {
        let conf = "[options]\nHoldPkg = pacman\n#DisableSandboxFilesystem\n#DisableSandboxSyscalls\n";
        assert_eq!(
            enable_pacman_option(conf, "DisableSandboxFilesystem").unwrap(),
            "[options]\nHoldPkg = pacman\nDisableSandboxFilesystem\n#DisableSandboxSyscalls\n"
        );
        assert_eq!(
            enable_pacman_option("[options]\nHoldPkg = pacman\n", "DisableSandboxFilesystem").unwrap(),
            "[options]\nDisableSandboxFilesystem\nHoldPkg = pacman\n"
        );
        assert!(enable_pacman_option("[options]\nDisableSandboxFilesystem\n", "DisableSandboxFilesystem").is_none());
        assert!(enable_pacman_option("[core]\n", "DisableSandboxFilesystem").is_none());
    }

    #[test]
    fn member_group_parsing() {
        let text = "wheel:x:10:root,alice\nandroid:x:1023:alice\nusers:x:100:\nbad\naudio:x:29:bob\n";
        assert_eq!(member_groups(text, "alice"), vec![10, 1023]);
        assert!(member_groups(text, "carol").is_empty());
    }

    #[test]
    fn session_env_validation() {
        assert!(validate_session_env(&["BAREBOOT_SSH_PORT=2222".into()]).is_ok());
        assert!(validate_session_env(&["LD_PRELOAD=/x.so".into()]).is_err());
        assert!(validate_session_env(&["BAREBOOT_X".into()]).is_err());
        assert!(validate_session_env(&["bareboot_x=1".into()]).is_err());
    }

    #[test]
    fn timezone_validation() {
        assert!(valid_timezone("Europe/Moscow"));
        assert!(valid_timezone("Etc/GMT+3"));
        assert!(valid_timezone("UTC"));
        assert!(!valid_timezone(""));
        assert!(!valid_timezone("/etc/passwd"));
        assert!(!valid_timezone("../../etc/shadow"));
        assert!(!valid_timezone("Europe//Moscow"));
        assert!(!valid_timezone("Europe/Mos cow"));
    }

    #[test]
    fn allowed_uids_parsing() {
        let parsed = parse_allowed_uids("# comment\n10234\n\n 10555 # app\nbad\n");
        assert_eq!(parsed, vec![10234, 10555]);
    }

    #[test]
    fn resolv_conf_fallback() {
        assert_eq!(
            format_resolv_conf(&[]),
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n"
        );
        assert_eq!(
            format_resolv_conf(&["9.9.9.9".into()]),
            "nameserver 9.9.9.9\n"
        );
    }
}
