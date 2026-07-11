use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use buffer::BufReaderDirectWriter;

pub mod buffer;
pub mod nmea;

pub enum Protocol {
    TCP,
    UDP,
    TCPListen,
    UDPListen,
    Serial,
}
impl std::str::FromStr for Protocol {
    type Err = std::io::Error;
    fn from_str(s: &str) -> io::Result<Self> {
        match s {
            "tcp" => Ok(Protocol::TCP),
            "udp" => Ok(Protocol::UDP),
            "tcp-listen" => Ok(Protocol::TCPListen),
            "udp-listen" => Ok(Protocol::UDPListen),
            "serial" => Ok(Protocol::Serial),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid protocol",
            )),
        }
    }
}
impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::TCP => write!(f, "tcp"),
            Protocol::UDP => write!(f, "udp"),
            Protocol::TCPListen => write!(f, "tcp-listen"),
            Protocol::UDPListen => write!(f, "udp-listen"),
            Protocol::Serial => write!(f, "serial"),
        }
    }
}
impl std::fmt::Debug for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

pub struct NetworkEndpoint {
    pub protocol: Protocol,
    pub addr: SocketAddr,
    pub tcp_listener: Option<std::net::TcpListener>,
    pub tcp_stream: Vec<BufReaderDirectWriter<std::net::TcpStream>>, // List of connected incoming TCP streams or single outgoing stream
    pub udp_socket: Option<std::net::UdpSocket>,
    pub serial_path: Option<String>, // Device path for Protocol::Serial
    pub serial_baud: u32,            // Baud rate for Protocol::Serial
    pub serial_reader: Option<io::BufReader<Box<dyn serialport::SerialPort>>>,
}

/// Standard AIS receivers emit NMEA-0183 at 38400 baud.
const DEFAULT_SERIAL_BAUD: u32 = 38400;

impl std::str::FromStr for NetworkEndpoint {
    type Err = std::io::Error;

    fn from_str(s: &str) -> std::io::Result<Self> {
        let parts = s.split("://").collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid address format, should be protocol://address",
            ));
        }
        let protocol = parts[0]
            .parse::<Protocol>()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;

        // Serial endpoints have no socket address; parse `path[:baud]` instead.
        // e.g. serial:///dev/ttyUSB0:38400  or  serial://COM3
        if let Protocol::Serial = protocol {
            let rest = parts[1];
            let (path, baud) = match rest.rsplit_once(':') {
                Some((p, b)) if !p.is_empty() && b.parse::<u32>().is_ok() => {
                    (p.to_string(), b.parse().unwrap())
                }
                _ => (rest.to_string(), DEFAULT_SERIAL_BAUD),
            };
            if path.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Serial endpoint requires a device path",
                ));
            }
            return Ok(NetworkEndpoint {
                protocol,
                addr: SocketAddr::from(([0, 0, 0, 0], 0)),
                tcp_listener: None,
                tcp_stream: Vec::new(),
                udp_socket: None,
                serial_path: Some(path),
                serial_baud: baud,
                serial_reader: None,
            });
        }

        let mut addr = parts[1].to_socket_addrs().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{}: {}", parts[1], e),
            )
        })?;
        let addr = addr.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "No address found")
        })?;
        Ok(NetworkEndpoint {
            protocol,
            addr,
            tcp_listener: None,
            tcp_stream: Vec::new(),
            udp_socket: None,
            serial_path: None,
            serial_baud: DEFAULT_SERIAL_BAUD,
            serial_reader: None,
        })
    }
}
impl std::fmt::Display for NetworkEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.protocol {
            Protocol::Serial => write!(
                f,
                "serial://{}:{}",
                self.serial_path.as_deref().unwrap_or("?"),
                self.serial_baud
            ),
            _ => write!(f, "{}://{}", self.protocol, self.addr),
        }
    }
}
impl std::fmt::Debug for NetworkEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}
impl std::convert::From<NetworkEndpoint> for SocketAddr {
    fn from(addr: NetworkEndpoint) -> Self {
        addr.addr
    }
}

pub fn send_message_udp(stream: &mut std::net::UdpSocket, message: &[u8]) -> std::io::Result<()> {
    stream.send(message)?;
    Ok(())
}

pub fn read_message_udp(stream: &mut std::net::UdpSocket) -> std::io::Result<String> {
    let mut buffer = vec![0; 1024];
    let (bytes_read, _) = stream.recv_from(&mut buffer)?;
    buffer.truncate(bytes_read);
    let buffer = String::from_utf8_lossy(&buffer).to_string();
    Ok(buffer)
}

pub fn send_message_tcp(
    stream: &mut BufReaderDirectWriter<TcpStream>,
    message: &[u8],
) -> std::io::Result<()> {
    stream.write_all(message)?;
    stream.flush()?;
    Ok(())
}

pub fn read_message_tcp(stream: &mut BufReaderDirectWriter<TcpStream>) -> io::Result<String> {
    let mut buffer = String::with_capacity(110);
    let bytes_read = stream.read_line(&mut buffer)?;
    buffer.truncate(bytes_read);
    Ok(buffer)
}

impl NetworkEndpoint {
    pub fn read_to_string(&mut self) -> io::Result<String> {
        match self.protocol {
            Protocol::TCP => {
                if self.tcp_stream.len() == 0 {
                    let stream = std::net::TcpStream::connect(self.addr).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::ConnectionRefused,
                            format!("provider {}: {}", self.addr, e),
                        )
                    })?;
                    log::info!("Connected to {}", self);
                    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                    let reader = BufReaderDirectWriter::new(stream);
                    self.tcp_stream.push(reader);
                }
                match read_message_tcp(&mut self.tcp_stream[0]) {
                    Ok(message) => {
                        if message.len() > 0 {
                            return Ok(message);
                        }
                        self.tcp_stream.clear();
                        return Err(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            "TCP stream closed",
                        ));
                    }
                    Err(e) => {
                        log::error!("Error reading from TCP stream: {}", e);
                        self.tcp_stream.clear();
                        return Err(e);
                    }
                }
            }
            Protocol::TCPListen => {
                if self.tcp_listener.is_none() {
                    let listener = TcpListener::bind(self.addr).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::AddrInUse,
                            format!("provider {}: {}", self.addr, e),
                        )
                    })?;
                    listener.set_nonblocking(true)?;
                    log::info!("Listening on: {}", self);
                    self.tcp_listener = Some(listener);
                }
                if let Some(tcp_listener) = self.tcp_listener.as_mut() {
                    loop {
                        match tcp_listener.accept() {
                            Ok((stream, addr)) => {
                                log::info!("Accepted connection from: {}", addr);
                                stream.set_nonblocking(true)?;
                                let reader = BufReaderDirectWriter::new(stream);
                                self.tcp_stream.push(reader);
                            }
                            Err(e) => {
                                if e.kind() == io::ErrorKind::WouldBlock {
                                    // No connection available, continue
                                    break;
                                }
                                log::error!("Error accepting connection: {}", e);
                                return Err(e);
                            }
                        }
                    }
                }

                self.tcp_stream.retain(|reader| {
                    if reader.peer_addr().is_err() {
                        log::warn!("Removing disconnected TCP stream");
                        false
                    } else {
                        true
                    }
                });

                let mut i = 0;
                while i < self.tcp_stream.len() {
                    match read_message_tcp(&mut self.tcp_stream[i]) {
                        Ok(message) => {
                            if message.len() > 0 {
                                return Ok(message);
                            }
                            // Drop stream on empty read
                            self.tcp_stream.remove(i);
                            continue;
                        }
                        Err(e) => {
                            match e.kind() {
                                io::ErrorKind::WouldBlock => {
                                    // No data available, continue
                                }
                                _ => {
                                    log::error!("Error reading from TCP stream: {}", e);
                                    self.tcp_stream.remove(i);
                                    if self.tcp_stream.is_empty() {
                                        return Err(e);
                                    }
                                }
                            }
                        }
                    }
                    i += 1;
                }
            }

            Protocol::UDP | Protocol::UDPListen => {
                if self.udp_socket.is_none() {
                    let socket = std::net::UdpSocket::bind(self.addr)?;
                    log::info!("Listening on: {}", self);
                    self.udp_socket = Some(socket);
                }
                if let Some(udp_socket) = self.udp_socket.as_mut() {
                    return read_message_udp(udp_socket);
                }
            }

            Protocol::Serial => {
                if self.serial_reader.is_none() {
                    let path = self.serial_path.clone().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "No serial device path")
                    })?;
                    let port = serialport::new(&path, self.serial_baud)
                        .timeout(Duration::from_secs(30))
                        .open()
                        .map_err(|e| {
                            io::Error::new(
                                io::ErrorKind::NotConnected,
                                format!("serial {}: {}", self, e),
                            )
                        })?;
                    log::info!("Opened {}", self);
                    self.serial_reader = Some(io::BufReader::new(port));
                }
                if let Some(reader) = self.serial_reader.as_mut() {
                    let mut buffer = String::with_capacity(110);
                    match reader.read_line(&mut buffer) {
                        Ok(0) => {
                            // EOF: device disappeared (e.g. USB unplugged). Drop and retry.
                            self.serial_reader = None;
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "serial device closed",
                            ));
                        }
                        Ok(_) => return Ok(buffer),
                        Err(e) => {
                            self.serial_reader = None;
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                format!("serial read {}: {}", self, e),
                            ));
                        }
                    }
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::Other,
            "Failed to read message from network endpoint",
        ))
    }

    pub fn is_connected(&mut self) -> bool {
        match self.protocol {
            Protocol::TCP | Protocol::TCPListen => {
                self.tcp_stream.retain(|writer| {
                    if writer.peer_addr().is_err() {
                        log::warn!("Removing disconnected TCP stream");
                        false
                    } else {
                        true
                    }
                });
                self.tcp_stream.len() > 0
            }
            Protocol::UDP => self.udp_socket.is_some(),
            Protocol::UDPListen => true,
            Protocol::Serial => self.serial_reader.is_some(),
        }
    }

    pub fn send_message(&mut self, nmea_message: &[u8], key: &String) -> io::Result<()> {
        match self.protocol {
            Protocol::TCP => {
                self.tcp_stream.retain(|writer| {
                    if writer.peer_addr().is_err() {
                        log::warn!("Removing disconnected TCP stream");
                        false
                    } else {
                        true
                    }
                });

                if self.tcp_stream.len() == 0 {
                    let stream =
                        std::net::TcpStream::connect_timeout(&self.addr, Duration::from_secs(30))
                            .map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::ConnectionRefused,
                                format!("{} ({}): {}", key, self.addr, e),
                            )
                        })?;

                    // Set the stream to use keepalive
                    let sock_ref = socket2::SockRef::from(&stream);
                    let mut ka = socket2::TcpKeepalive::new();
                    ka = ka.with_time(Duration::from_secs(30));
                    ka = ka.with_interval(Duration::from_secs(30));
                    sock_ref.set_tcp_keepalive(&ka)?;

                    sock_ref.set_read_timeout(Some(Duration::from_secs(30)))?;
                    sock_ref.set_write_timeout(Some(Duration::from_secs(30)))?;

                    log::info!("{}: Connected to {}", key, self);
                    let writer = BufReaderDirectWriter::new(stream);
                    self.tcp_stream.push(writer);
                }
                if let Some(tcp_stream) = self.tcp_stream.get_mut(0) {
                    send_message_tcp(tcp_stream, nmea_message).map_err(|e| {
                        self.tcp_stream.clear();
                        std::io::Error::new(
                            std::io::ErrorKind::ConnectionRefused,
                            format!("send_message tcp {} ({}): {}", key, self.addr, e),
                        )
                    })?;
                    log::debug!("{}: Sent message to {}", key, self);
                }
            }
            Protocol::UDP => {
                if self.udp_socket.is_none() {
                    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::ConnectionRefused,
                            format!("{} ({}): {}", key, self.addr, e),
                        )
                    })?;
                    UdpSocket::connect(&socket, self.addr)?;
                    log::info!("{}: Connected to {}", key, self);
                    self.udp_socket = Some(socket);
                }
                if let Some(udp_socket) = self.udp_socket.as_mut() {
                    send_message_udp(udp_socket, nmea_message).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::ConnectionRefused,
                            format!("send_message udp {} ({}): {}", key, self.addr, e),
                        )
                    })?;
                }
            }
            // Listeners and serial inputs are receive-only; nothing to send.
            Protocol::TCPListen | Protocol::UDPListen | Protocol::Serial => {}
        }
        Ok(())
    }
}
