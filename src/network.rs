use crate::protocol::PORT;
use std::{
    io,
    mem::{size_of, zeroed},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    os::windows::io::AsRawSocket,
};
use windows_sys::Win32::{
    NetworkManagement::IpHelper::{GetIfEntry2, MIB_IF_ROW2},
    Networking::WinSock::{setsockopt, WSAGetLastError, IPPROTO_IP, IP_UNICAST_IF, SOCKET_ERROR},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
    pub index: u32,
    pub hardware: bool,
}

impl Interface {
    fn mask(&self) -> u32 {
        u32::MAX << (32 - self.prefix)
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        u32::from(ip) & self.mask() == u32::from(self.ip) & self.mask()
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        // if-addrs 0.13 can take the first Windows prefix (/32), even though
        // OnLinkPrefixLength is /24. Calculate from the actual on-link prefix.
        Ipv4Addr::from(u32::from(self.ip) | !self.mask())
    }
}

pub fn interfaces() -> io::Result<Vec<Interface>> {
    let mut result = Vec::new();
    for interface in if_addrs::get_if_addrs()? {
        let if_addrs::IfAddr::V4(address) = interface.addr else {
            continue;
        };
        let Some(index) = interface.index else {
            continue;
        };
        if address.ip.is_loopback()
            || address.ip.is_unspecified()
            || !(1..=30).contains(&address.prefixlen)
        {
            continue;
        }
        let mut row: MIB_IF_ROW2 = unsafe { zeroed() };
        row.InterfaceIndex = index;
        if unsafe { GetIfEntry2(&mut row) } != 0 || row.OperStatus != 1 {
            continue;
        }
        // HardwareInterface is bit zero of the documented native bit field.
        let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
        result.push(Interface {
            name: interface.name,
            ip: address.ip,
            prefix: address.prefixlen,
            index,
            hardware,
        });
    }
    // Physical LANs take priority over TAP/WireGuard/TUN, independent of VPN brand.
    // Virtual Ethernet bridges remain usable if no physical LAN owns an IPv4 address.
    if result.iter().any(|interface| interface.hardware) {
        result.retain(|interface| interface.hardware);
    }
    result.sort_by_key(|interface| (interface.index, interface.ip));
    result.dedup();
    Ok(result)
}

pub fn route(interfaces: &[Interface], destination: Ipv4Addr) -> u32 {
    interfaces
        .iter()
        .filter(|interface| interface.contains(destination))
        .max_by_key(|interface| interface.prefix)
        .map_or(0, |interface| interface.index)
}

pub fn set_interface(socket: &UdpSocket, index: u32) -> io::Result<()> {
    // Winsock requires the interface index in NETWORK byte order for this option.
    let value = index.to_be();
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            IPPROTO_IP,
            IP_UNICAST_IF,
            (&value as *const u32).cast(),
            size_of::<u32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }))
    } else {
        Ok(())
    }
}

pub fn targets(interfaces: &[Interface]) -> Vec<SocketAddr> {
    let mut result = Vec::new();
    for interface in interfaces {
        // At most one /24-sized on-link block per adapter. Never walk a VPN's
        // 10.0.0.0/8 or a large corporate LAN. Manual IP covers other blocks.
        let mask = u32::MAX << (32 - interface.prefix.max(24));
        let base = u32::from(interface.ip) & mask;
        let end = base | !mask;
        let network = u32::from(interface.ip) & interface.mask();
        for host in base..=end {
            let ip = Ipv4Addr::from(host);
            if host == network
                || ip == interface.broadcast()
                || interfaces.iter().any(|i| i.ip == ip)
            {
                continue;
            }
            let target = SocketAddr::from((ip, PORT));
            if !result.contains(&target) {
                result.push(target);
            }
        }
    }
    result
}

pub fn validate_peer(text: &str) -> Result<Option<Ipv4Addr>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let ip: Ipv4Addr = text.parse().map_err(|_| "Введите IPv4 второго компьютера, например 192.168.0.25. Пустое поле включает автопоиск.".to_string())?;
    if ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.octets()[0] == 0
    {
        return Err(
            "Нужен локальный IPv4 другого компьютера, а не адрес VPN, роутера или 127.0.0.1."
                .into(),
        );
    }
    Ok(Some(ip))
}

pub fn validate_local_peer(ip: Ipv4Addr, interfaces: &[Interface]) -> Result<(), String> {
    if interfaces.iter().any(|interface| interface.ip == ip) {
        return Err(
            "Это адрес этого компьютера. Введите IP второго компьютера из его меню «Подключение»."
                .into(),
        );
    }
    if !interfaces.iter().any(|interface| {
        interface.contains(ip)
            && interface.broadcast() != ip
            && u32::from(ip) != u32::from(interface.ip) & interface.mask()
    }) {
        return Err("Адрес вне вашей локальной сети. Нужен IP Wi-Fi/Ethernet второго компьютера из его меню «Подключение», а не IP VPN.".into());
    }
    Ok(())
}

fn settings_path() -> io::Result<std::path::PathBuf> {
    Ok(std::env::current_exe()?.with_file_name("wifiradio.peer"))
}

pub fn load_peer() -> Option<Ipv4Addr> {
    settings_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| validate_peer(&text).ok().flatten())
}

pub fn save_peer(peer: Option<Ipv4Addr>) -> io::Result<()> {
    std::fs::write(
        settings_path()?,
        peer.map_or(String::new(), |ip| ip.to_string()),
    )
}

pub fn report() -> String {
    let mut text = format!("WifiRadio {}\r\nUDP {PORT}\r\n", env!("CARGO_PKG_VERSION"));
    match interfaces() {
        Ok(interfaces) => {
            for interface in &interfaces {
                text.push_str(&format!(
                    "LAN: {} {}/{}; interface={}; broadcast={}\r\n",
                    interface.name,
                    interface.ip,
                    interface.prefix,
                    interface.index,
                    interface.broadcast()
                ));
                if let Ok(raw) = if_addrs::get_if_addrs() {
                    for adapter in raw {
                        if let if_addrs::IfAddr::V4(address) = adapter.addr {
                            if address.ip == interface.ip {
                                text.push_str(&format!(
                                    "Library broadcast: {:?}; library netmask: {}\r\n",
                                    address.broadcast, address.netmask
                                ));
                            }
                        }
                    }
                }
                match UdpSocket::bind((interface.ip, 0)) {
                    Ok(socket) => match set_interface(&socket, interface.index) {
                        Ok(()) => {
                            text.push_str("Interface routing: OK\r\n");
                        }
                        Err(error) => text.push_str(&format!("Interface routing: {error}\r\n")),
                    },
                    Err(error) => text.push_str(&format!("Bind: {error}\r\n")),
                }
            }
            text.push_str(&format!(
                "Unicast discovery targets: {}\r\n",
                targets(&interfaces).len()
            ));
        }
        Err(error) => text.push_str(&format!("Adapters: {error}\r\n")),
    }
    text.push_str("No microphone, firewall, VPN, or startup changes.\r\n");
    text
}

pub fn destination_ip(address: SocketAddr) -> Option<Ipv4Addr> {
    match address.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan() -> Interface {
        Interface {
            name: "Ethernet".into(),
            ip: Ipv4Addr::new(192, 168, 0, 112),
            prefix: 24,
            index: 14,
            hardware: true,
        }
    }

    #[test]
    fn broadcast_uses_on_link_prefix_and_never_a_host_prefix() {
        assert_eq!(lan().broadcast(), Ipv4Addr::new(192, 168, 0, 255));
        let mut interface = lan();
        interface.prefix = 23;
        assert_eq!(interface.broadcast(), Ipv4Addr::new(192, 168, 1, 255));
    }

    #[test]
    fn unicast_search_is_bounded_to_local_hosts_and_excludes_self() {
        let targets = targets(&[lan()]);
        assert_eq!(targets.len(), 253);
        assert!(!targets.contains(&SocketAddr::from((lan().ip, PORT))));
        assert!(!targets.contains(&SocketAddr::from((lan().broadcast(), PORT))));
        let mut large = lan();
        large.ip = Ipv4Addr::new(10, 0, 12, 13);
        large.prefix = 8;
        let targets = super::targets(&[large]);
        assert_eq!(targets.len(), 255);
        assert!(targets
            .iter()
            .all(|target| target.ip().to_string().starts_with("10.0.12.")));
    }

    #[test]
    fn choose_the_local_interface_instead_of_the_default_vpn_route() {
        assert_eq!(route(&[lan()], Ipv4Addr::new(192, 168, 0, 42)), 14);
        assert_eq!(route(&[lan()], Ipv4Addr::LOCALHOST), 0);
    }

    #[test]
    fn manual_peer_accepts_ipv4_or_empty_and_rejects_special_addresses() {
        assert_eq!(
            validate_peer(" 192.168.0.25 ").unwrap(),
            Some(Ipv4Addr::new(192, 168, 0, 25))
        );
        assert_eq!(validate_peer(" ").unwrap(), None);
        for invalid in [
            "127.0.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "example.com",
            "192.168.0.2:49735",
        ] {
            assert!(validate_peer(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn direct_peer_must_be_another_host_on_the_selected_lan() {
        assert!(validate_local_peer(Ipv4Addr::new(192, 168, 0, 25), &[lan()]).is_ok());
        for invalid in [
            lan().ip,
            lan().broadcast(),
            Ipv4Addr::new(192, 168, 0, 0),
            Ipv4Addr::new(10, 8, 1, 1),
            Ipv4Addr::new(192, 168, 1, 25),
        ] {
            assert!(validate_local_peer(invalid, &[lan()]).is_err(), "{invalid}");
        }
    }
}
