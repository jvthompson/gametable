//! Best-effort automatic port forwarding through the router (UPnP IGD: SSDP discovery and SOAP
//! AddPortMapping / DeletePortMapping), so hosting over the internet doesn't need a manual router
//! rule. Port of `src/net/upnp.lua` and `upnp_worker.lua`. The round-trips run on a worker thread;
//! every failure means "not mapped".

use std::net::{IpAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::plain_http;

const SSDP_ADDRESS: &str = "239.255.255.250:1900";
const SERVICE_TYPES: [&str; 2] =
    ["urn:schemas-upnp-org:service:WANIPConnection:1", "urn:schemas-upnp-org:service:WANPPPConnection:1"];
const SEARCH_MESSAGE: &str = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: upnp:rootdevice\r\n\r\n";
const HTTP_TIMEOUT: Duration = Duration::from_secs(4);

/// A mapping the router accepted (pass it to [`release_mapping`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PortMapping {
    pub port: u16,
    pub control_url: String,
    pub service_type: String,
}

/// The LOCATION header of an SSDP response.
pub fn parse_location(response: &str) -> Option<String> {
    response.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.eq_ignore_ascii_case("location") {
            return None;
        }
        value.split_whitespace().next().map(str::to_string)
    })
}

/// "scheme://authority" and the path ("/" when absent).
fn split_url(url: &str) -> Option<(String, String)> {
    let scheme_end = url.find("://")?;
    let after = &url[scheme_end + 3..];
    let slash = after.find('/').unwrap_or(after.len());
    let root = url[..scheme_end + 3 + slash].to_string();
    let path = if slash == after.len() { "/".to_string() } else { after[slash..].to_string() };
    Some((root, path))
}

/// Resolves a (possibly relative) controlURL against the description URL.
pub fn resolve_url(base: &str, reference: &str) -> String {
    if reference.contains("://") {
        return reference.to_string();
    }
    let Some((root, path)) = split_url(base) else { return reference.to_string() };
    if reference.starts_with('/') {
        return format!("{root}{reference}");
    }
    let directory = path.rfind('/').map_or("/", |slash| &path[..=slash]);
    format!("{root}{directory}{reference}")
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// The control URL and service type of the first WANIPConnection (then WANPPPConnection) service,
/// looked up within its own `<service>` block.
pub fn find_control_url(xml: &str, location: &str) -> Option<(String, String)> {
    for service_type in SERVICE_TYPES {
        let mut rest = xml;
        while let Some(block) = between(rest, "<service>", "</service>") {
            let declared = between(block, "<serviceType>", "</serviceType>").map(str::trim);
            if declared == Some(service_type)
                && let Some(control) = between(block, "<controlURL>", "</controlURL>").map(str::trim)
                && !control.is_empty()
            {
                return Some((resolve_url(location, control), service_type.to_string()));
            }
            let consumed = rest.find("</service>").map_or(rest.len(), |end| end + "</service>".len());
            rest = &rest[consumed..];
        }
    }
    None
}

fn soap_envelope(service_type: &str, action: &str, fields: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service_type}\">\
         {fields}</u:{action}></s:Body></s:Envelope>"
    )
}

fn add_fields(port: u16, internal_client: &str, description: &str) -> String {
    format!(
        "<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>TCP</NewProtocol>\
         <NewInternalPort>{port}</NewInternalPort><NewInternalClient>{internal_client}</NewInternalClient>\
         <NewEnabled>1</NewEnabled><NewPortMappingDescription>{description}</NewPortMappingDescription>\
         <NewLeaseDuration>0</NewLeaseDuration>"
    )
}

fn delete_fields(port: u16) -> String {
    format!("<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>TCP</NewProtocol>")
}

/// SSDP M-SEARCH, listening the whole window (TVs and printers answer too).
fn discover_locations(window: Duration) -> Vec<String> {
    let mut locations = Vec::new();
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else { return locations };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    if socket.send_to(SEARCH_MESSAGE.as_bytes(), SSDP_ADDRESS).is_err() {
        return locations;
    }
    let deadline = Instant::now() + window;
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        if let Ok((read, _)) = socket.recv_from(&mut buffer)
            && let Some(location) = parse_location(&String::from_utf8_lossy(&buffer[..read]))
            && !locations.contains(&location)
        {
            locations.push(location);
        }
    }
    locations
}

fn soap(control_url: &str, service_type: &str, action: &str, fields: &str) -> bool {
    let body = soap_envelope(service_type, action, fields);
    let headers = [
        ("content-type", "text/xml; charset=\"utf-8\"".to_string()),
        ("soapaction", format!("\"{service_type}#{action}\"")),
    ];
    // A refusal is a SOAP Fault under HTTP 500; only the status matters.
    plain_http::request("POST", control_url, &headers, body.as_bytes(), HTTP_TIMEOUT)
        .is_ok_and(|response| response.status == 200)
}

/// The local address the OS routes through to reach the gateway (what the router forwards to).
fn local_address_for(host: &str) -> Option<String> {
    let gateway = (host, 1900).to_socket_addrs().ok()?.find(|address| address.is_ipv4())?;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(gateway).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip.to_string()),
        _ => None,
    }
}

fn add_mapping(port: u16, description: &str) -> Option<PortMapping> {
    for location in discover_locations(Duration::from_secs(3)) {
        let xml = plain_http::request("GET", &location, &[], &[], HTTP_TIMEOUT)
            .ok()
            .filter(|response| response.status == 200)
            .map(|response| String::from_utf8_lossy(&response.body).to_string());
        let Some((control_url, service_type)) = xml.and_then(|xml| find_control_url(&xml, &location)) else {
            continue;
        };
        let host = plain_http::split_http_url(&control_url).map(|(host, _, _)| host);
        let internal = host.and_then(|host| local_address_for(&host));
        if let Some(internal) = internal
            && soap(&control_url, &service_type, "AddPortMapping", &add_fields(port, &internal, description))
        {
            return Some(PortMapping { port, control_url, service_type });
        }
        // The first real gateway decides.
        return None;
    }
    None
}

/// A mapping request in flight.
pub struct MappingRequest {
    result: Receiver<Option<PortMapping>>,
}

impl MappingRequest {
    /// `U.requestMapping(port)`: asks the gateway to forward TCP `port` to this machine.
    pub fn start(port: u16, description: &str) -> MappingRequest {
        let (sender, result) = mpsc::channel();
        let description = description.to_string();
        std::thread::spawn(move || {
            let _ = sender.send(add_mapping(port, &description));
        });
        MappingRequest { result }
    }

    /// `None` while asking; then `Some(mapping or None)` once.
    pub fn poll(&mut self) -> Option<Option<PortMapping>> {
        match self.result.try_recv() {
            Ok(mapping) => Some(mapping),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }
}

/// Fire-and-forget removal of a mapping.
pub fn release_mapping(mapping: PortMapping) {
    std::thread::spawn(move || {
        soap(&mapping.control_url, &mapping.service_type, "DeletePortMapping", &delete_fields(mapping.port));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_descriptions() {
        let response =
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
        assert_eq!(parse_location(response).as_deref(), Some("http://192.168.1.1:5000/rootDesc.xml"));
        let xml = "<root><service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>\
                   <controlURL>/ctl/L3F</controlURL></service><service><serviceType>\
                   urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL> ctl/IPConn </controlURL>\
                   </service></root>";
        assert_eq!(
            find_control_url(xml, "http://192.168.1.1:5000/desc/rootDesc.xml"),
            Some((
                "http://192.168.1.1:5000/desc/ctl/IPConn".to_string(),
                "urn:schemas-upnp-org:service:WANIPConnection:1".to_string()
            ))
        );
        assert_eq!(resolve_url("http://a:1/x/y.xml", "/ctl"), "http://a:1/ctl");
        assert_eq!(resolve_url("http://a:1", "ctl"), "http://a:1/ctl");
        assert!(
            add_fields(51234, "192.168.1.5", "Cardtable")
                .contains("<NewInternalClient>192.168.1.5</NewInternalClient>")
        );
    }
}
