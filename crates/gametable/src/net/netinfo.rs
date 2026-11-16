//! This machine's addresses for the Host Setup screen. Port of `src/net/netinfo.lua`.

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use super::plain_http;

/// `N.isIPv4`: four dot-separated numbers, each at most 255.
pub fn is_ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part.parse::<u32>().is_ok_and(|n| n <= 255)
        })
}

/// Every non-loopback IPv4 address (best effort): the one the OS routes outward with, then the
/// host name's addresses.
pub fn local_ipv4_addresses() -> Vec<String> {
    let mut addresses: Vec<String> = Vec::new();
    let mut add = |ip: Ipv4Addr| {
        let text = ip.to_string();
        if !ip.is_loopback() && !ip.is_unspecified() && !addresses.contains(&text) {
            addresses.push(text);
        }
    };
    // No packet is sent; connecting a UDP socket just picks a route.
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:53").is_ok()
        && let Ok(IpAddr::V4(ip)) = socket.local_addr().map(|address| address.ip())
    {
        add(ip);
    }
    let host_name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
    if let Some(addresses) = host_name.and_then(|name| (name.as_str(), 0).to_socket_addrs().ok()) {
        for address in addresses {
            if let IpAddr::V4(ip) = address.ip() {
                add(ip);
            }
        }
    }
    addresses
}

/// The public IPv4 from api.ipify.org (plain HTTP: the answer is just our own address), fetched
/// on a worker thread. Poll [`PublicAddressLookup::poll`] each frame.
pub struct PublicAddressLookup {
    result: Receiver<Option<String>>,
}

/// What a lookup reports.
#[derive(Debug, Clone, PartialEq)]
pub enum LookupProgress {
    Pending,
    Done(Option<String>),
}

impl PublicAddressLookup {
    pub fn start() -> PublicAddressLookup {
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            let response = plain_http::request("GET", "http://api.ipify.org", &[], &[], Duration::from_secs(5));
            let ip = response
                .ok()
                .filter(|response| response.status == 200)
                .map(|response| String::from_utf8_lossy(&response.body).split_whitespace().collect::<String>())
                .filter(|ip| is_ipv4(ip));
            let _ = sender.send(ip);
        });
        PublicAddressLookup { result }
    }

    pub fn poll(&mut self) -> LookupProgress {
        match self.result.try_recv() {
            Ok(ip) => LookupProgress::Done(ip),
            Err(TryRecvError::Empty) => LookupProgress::Pending,
            Err(TryRecvError::Disconnected) => LookupProgress::Done(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_text() {
        assert!(is_ipv4("192.168.1.42"));
        assert!(!is_ipv4("256.1.1.1"));
        assert!(!is_ipv4("1.2.3"));
        assert!(!is_ipv4("a.b.c.d"));
        assert!(local_ipv4_addresses().iter().all(|ip| is_ipv4(ip) && !ip.starts_with("127.")));
    }
}
