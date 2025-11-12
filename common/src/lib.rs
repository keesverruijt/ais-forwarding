use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

pub mod buffer;
use buffer::BufReaderDirectWriter;

pub enum Protocol {
    TCP,
    UDP,
    TCPListen,
    UDPListen,
}
impl std::str::FromStr for Protocol {
    type Err = std::io::Error;
    fn from_str(s: &str) -> io::Result<Self> {
        match s {
            "tcp" => Ok(Protocol::TCP),
            "udp" => Ok(Protocol::UDP),
            "tcp-listen" => Ok(Protocol::TCPListen),
            "udp-listen" => Ok(Protocol::UDPListen),
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
        }
    }
}
impl std::fmt::Debug for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::TCP => write!(f, "tcp"),
            Protocol::UDP => write!(f, "udp"),
            Protocol::TCPListen => write!(f, "tcp-listen"),
            Protocol::UDPListen => write!(f, "udp-listen"),
        }
    }
}

pub struct NetworkEndpoint {
    pub protocol: Protocol,
    pub addr: SocketAddr,
    pub tcp_listener: Option<std::net::TcpListener>,
    pub tcp_stream: Vec<BufReaderDirectWriter<std::net::TcpStream>>, // List of connected incoming TCP streams or single outgoing stream
    pub udp_socket: Option<std::net::UdpSocket>,
}

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
        })
    }
}
impl std::fmt::Display for NetworkEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.protocol, self.addr)
    }
}
impl std::fmt::Debug for NetworkEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.protocol, self.addr)
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
                    let stream = std::net::TcpStream::connect(self.addr).map_err(|e| {
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
            Protocol::TCPListen | Protocol::UDPListen => {}
        }
        Ok(())
    }
}
