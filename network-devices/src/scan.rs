use std::collections::HashMap;
use std::fmt;
use std::net::{Ipv4Addr, UdpSocket};
use std::str::FromStr;

use serde::Deserialize;

/// Kept out of the sweep: larger networks would mean thousands of probes per scan.
const SMALLEST_PREFIX: u8 = 22;
/// The discard service; whether anything listens does not matter, only the address resolution.
const PROBE_PORT: u16 = 9;
/// `ATF_COM` in `/proc/net/arp`: the address answered; without it the entry is incomplete or failed.
const COMPLETE_FLAG: u32 = 0x2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct Mac([u8; 6]);

impl FromStr for Mac {
    type Err = String;

    /// `aa:bb:cc:dd:ee:ff` or `AA-BB-CC-DD-EE-FF`.
    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("invalid MAC address `{text}`");
        let parts: Vec<&str> = text.split([':', '-']).collect();
        if parts.len() != 6 {
            return Err(invalid());
        }
        let mut bytes = [0; 6];
        for (byte, part) in bytes.iter_mut().zip(parts) {
            if part.len() != 2 {
                return Err(invalid());
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
        }
        Ok(Mac(bytes))
    }
}

impl TryFrom<String> for Mac {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let [a, b, c, d, e, f] = self.0;
        write!(formatter, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Subnet {
    network: u32,
    prefix: u8,
}

impl Subnet {
    fn mask(self) -> u32 {
        u32::MAX << (32 - self.prefix)
    }

    pub fn contains(self, address: Ipv4Addr) -> bool {
        u32::from(address) & self.mask() == self.network
    }

    /// Every address but the network and broadcast ones.
    pub fn hosts(self) -> impl Iterator<Item = Ipv4Addr> {
        let broadcast = self.network | !self.mask();
        (self.network + 1..broadcast).map(Ipv4Addr::from)
    }
}

impl FromStr for Subnet {
    type Err = String;

    /// `192.168.15.0/24`, from /22 to /30.
    fn from_str(text: &str) -> Result<Self, String> {
        let (address, prefix) = text
            .split_once('/')
            .ok_or_else(|| format!("subnet `{text}` needs a prefix, e.g. /24"))?;
        let address: Ipv4Addr = address
            .parse()
            .map_err(|_| format!("invalid address in subnet `{text}`"))?;
        let prefix: u8 = prefix
            .parse()
            .ok()
            .filter(|prefix| (SMALLEST_PREFIX..=30).contains(prefix))
            .ok_or_else(|| {
                format!("subnet `{text}`: the prefix goes from /{SMALLEST_PREFIX} to /30")
            })?;
        let subnet = Subnet {
            network: u32::from(address),
            prefix,
        };
        if subnet.network & !subnet.mask() != 0 {
            return Err(format!("subnet `{text}` has host bits set"));
        }
        Ok(subnet)
    }
}

impl TryFrom<String> for Subnet {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
    }
}

impl fmt::Display for Subnet {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "{}/{}",
            Ipv4Addr::from(self.network),
            self.prefix
        )
    }
}

/// One UDP datagram to every host: the kernel resolves each address, confirming the entries of
/// devices still there and failing the ones that left. No root needed, unlike ping or ARP.
pub fn probe(subnet: Subnet) -> std::io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_nonblocking(true)?;
    for host in subnet.hosts() {
        // Unreachable hosts fail here or later; either way their entry tells.
        let _ = socket.send_to(&[0], (host, PROBE_PORT));
    }
    Ok(())
}

/// The devices of the subnet that answered and their addresses, from `/proc/net/arp`.
pub fn answered(subnet: Subnet) -> std::io::Result<HashMap<Mac, Ipv4Addr>> {
    Ok(parse_arp_table(
        &std::fs::read_to_string("/proc/net/arp")?,
        subnet,
    ))
}

fn parse_arp_table(table: &str, subnet: Subnet) -> HashMap<Mac, Ipv4Addr> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let address: Ipv4Addr = columns.next()?.parse().ok()?;
            let _hardware_type = columns.next()?;
            let flags = u32::from_str_radix(columns.next()?.trim_start_matches("0x"), 16).ok()?;
            let mac: Mac = columns.next()?.parse().ok()?;
            (subnet.contains(address) && flags & COMPLETE_FLAG != 0).then_some((mac, address))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_addresses_accept_both_separators_and_any_case() {
        let mac: Mac = "08-D1-F9-ca-41-e4".parse().unwrap();
        assert_eq!(mac.to_string(), "08:d1:f9:ca:41:e4");
        assert!("08:d1:f9:ca:41".parse::<Mac>().is_err());
        assert!("08:d1:f9:ca:41:e".parse::<Mac>().is_err());
        assert!("08:d1:f9:ca:41:zz".parse::<Mac>().is_err());
    }

    #[test]
    fn subnets_list_their_hosts() {
        let subnet: Subnet = "192.168.15.0/24".parse().unwrap();
        let hosts: Vec<_> = subnet.hosts().collect();
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 15, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(192, 168, 15, 254));
        assert!(subnet.contains(Ipv4Addr::new(192, 168, 15, 77)));
        assert!(!subnet.contains(Ipv4Addr::new(192, 168, 16, 77)));
    }

    #[test]
    fn subnets_too_large_or_misaligned_are_refused() {
        assert!("192.168.15.0".parse::<Subnet>().is_err());
        assert!("10.0.0.0/8".parse::<Subnet>().is_err());
        assert!("192.168.15.1/24".parse::<Subnet>().is_err());
        assert!("192.168.12.0/22".parse::<Subnet>().is_ok());
    }

    #[test]
    fn only_complete_entries_of_the_subnet_count() {
        let table = "\
IP address       HW type     Flags       HW address            Mask     Device
172.17.0.2       0x1         0x2         fa:ce:9e:a2:45:44     *        docker0
192.168.15.100   0x1         0x2         bc:24:11:20:56:6d     *        wlan0
192.168.15.6     0x1         0x2         08:d1:f9:ca:41:e4     *        wlan0
192.168.15.40    0x1         0x0         00:00:00:00:00:00     *        wlan0
";
        let answered = parse_arp_table(table, "192.168.15.0/24".parse().unwrap());
        assert_eq!(answered.len(), 2);
        assert_eq!(
            answered[&"08:d1:f9:ca:41:e4".parse().unwrap()],
            Ipv4Addr::new(192, 168, 15, 6)
        );
        assert!(!answered.contains_key(&"fa:ce:9e:a2:45:44".parse().unwrap()));
    }
}
