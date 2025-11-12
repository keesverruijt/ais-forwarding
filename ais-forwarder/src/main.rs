use clap::Parser;
use config::Config;
use env_logger::Env;
use nmea_parser::ParsedMessage;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::ops::Add;
use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc::Sender;
use std::thread::Builder;
use std::time::{Duration, Instant, SystemTime};
use std::{io, path};

use common::NetworkEndpoint;
use common::Protocol;
use common::buffer::BufReaderDirectWriter;
use common::send_message_tcp;
use common::send_message_udp;

mod cache;
mod location;

#[derive(PartialEq)]
enum RmcSource {
    RMC,
    AIS,
    Both,
}

struct LastSent {
    vessel_dynamic_data: Instant,
    vessel_static_data: Instant,
}

struct Dispatcher {
    provider: NetworkEndpoint,
    ais: HashMap<String, NetworkEndpoint>,
    location_tx: Sender<ParsedMessage>,
    interval: u64,
    location_interval: u64,
    location_anchor_interval: u64,
    nmea_parser: nmea_parser::NmeaParser,
    last_sent: HashMap<u32, LastSent>,
    last_sent_location: SystemTime,
}

#[derive(Parser, Clone, Debug)]
pub struct Cli {
    #[clap(flatten)]
    pub verbose: clap_verbosity_flag::Verbosity<clap_verbosity_flag::InfoLevel>,

    /// Configuration file (supports .ini, .toml, .json, .yaml) --
    /// If the file is relative, it will be searched in /etc/ais-forwarder or /usr/local/etc/ais-forwarder.
    /// If the file is absolute, it will be used as is.
    #[clap(long, default_value = "config")]
    pub config: String,

    /// Cache directory --
    /// This must be a directory that is writable by the user running the program.
    /// If the directory does not exist, it will be created.
    #[clap(long, default_value = "/usr/local/var/cache/ais-forwarder")]
    pub cache_dir: String,
}

fn main() {
    let cli = Cli::parse();
    let log_level = cli.verbose.log_level_filter();
    let mut logger = env_logger::Builder::from_env(Env::default());
    logger.filter_level(log_level);
    // When running as a procd daemon, the PWD environment variable is not set
    // which can be used to shorten the logging records that already contain the timestamp.
    if std::env::var("PWD").is_err() {
        logger.format_timestamp(None);
    }
    logger.init();

    let mut config_path = PathBuf::from(cli.config);
    if config_path.is_relative() {
        config_path = get_config_dir().join(config_path);
    }
    let config_path = config_path
        .to_str()
        .expect("Cannot convert config path to string");
    log::info!("Loading config from {}", config_path);

    let settings = match Config::builder()
        .add_source(config::File::with_name(config_path))
        .build()
    {
        Ok(config) => config,
        Err(e) => {
            log::error!("Error loading {}: {}", config_path, e);
            exit(1);
        }
    };

    let settings = match settings.try_deserialize::<HashMap<String, HashMap<String, String>>>() {
        Ok(config) => config,
        Err(e) => {
            log::error!("Invalid format in {}: {}", config_path, e);
            exit(1);
        }
    };
    log::info!("Settings: {:?}", settings);

    let general = match settings.get("general") {
        Some(internal) => internal,
        None => {
            log::error!("Missing [internal] section in config.ini");
            exit(1);
        }
    };
    let mmsi = match general.get("mmsi").map(|v| v.parse::<u32>()) {
        None => {
            log::error!("Missing MMSI in config.ini");
            exit(1);
        }
        Some(Ok(interval)) => interval,
        Some(Err(e)) => {
            log::error!("Invalid MMSI in config.ini: {}", e);
            exit(1);
        }
    };

    let rmc_source = match general.get("rmc_source") {
        None => {
            log::error!("Missing rmc_source in config.ini");
            exit(1);
        }
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "rmc" => RmcSource::RMC,
            "ais" => RmcSource::AIS,
            "both" => RmcSource::Both,
            _ => {
                log::error!("Invalid rmc_source (should be AIS, RMC or Both) in config.ini");
                exit(1);
            }
        },
    };
    let interval = match general.get("interval").map(|v| v.parse::<u64>()) {
        None => 60,
        Some(Ok(interval)) => interval,
        Some(Err(e)) => {
            log::error!("Invalid interval in config.ini: {}", e);
            exit(1);
        }
    };
    let location_interval = match general.get("location_interval").map(|v| v.parse::<u64>()) {
        None => 600,
        Some(Ok(interval)) => interval,
        Some(Err(e)) => {
            log::error!("Invalid location_interval in config.ini: {}", e);
            exit(1);
        }
    };
    let location_anchor_interval = match general
        .get("location_anchor_interval")
        .map(|v| v.parse::<u64>())
    {
        None => 86400,
        Some(Ok(interval)) => interval,
        Some(Err(e)) => {
            log::error!("Invalid location_anchor_interval in config.ini: {}", e);
            exit(1);
        }
    };

    let (tx, rx) = std::sync::mpsc::channel::<ParsedMessage>();
    let location = match settings.get("location") {
        Some(location) => location,
        None => {
            log::error!("Missing [location] section in config.ini");
            exit(1);
        }
    }
    .into_iter()
    .map(|(key, value)| {
        let address = value
            .parse::<NetworkEndpoint>()
            .map_err(|e| {
                log::error!("Invalid address '{}' in config.ini: {}", value, e);
                exit(1);
            })
            .unwrap();
        (key.clone(), address)
    })
    .collect();
    Builder::new()
        .name("location".to_string())
        .spawn(move || {
            location::work_thread(rx, location, mmsi, cli.cache_dir.as_str());
        })
        .unwrap();

    loop {
        let provider = match general
            .get("provider")
            .map(|v| v.parse::<NetworkEndpoint>())
        {
            None => {
                log::error!("Missing provider in config.ini");
                exit(1);
            }
            Some(Ok(provider)) => provider,
            Some(Err(e)) => {
                log::error!("Invalid interval in config.ini: {}", e);
                exit(1);
            }
        };

        let ais = match settings.get("ais") {
            Some(ais) => ais,
            None => {
                log::error!("Missing [ais] section in config.ini");
                exit(1);
            }
        };
        let ais = ais
            .into_iter()
            .map(|(key, value)| {
                let address = value
                    .parse::<NetworkEndpoint>()
                    .map_err(|e| {
                        log::error!("Invalid address '{}' in config.ini: {}", value, e);
                        exit(1);
                    })
                    .unwrap();
                (key.clone(), address)
            })
            .collect();

        let mut dispatcher = Dispatcher::new(
            provider,
            ais,
            tx.clone(),
            interval,
            location_interval,
            location_anchor_interval,
        );
        if let Err(e) = dispatcher.work(&rmc_source) {
            log::error!("{}", e);
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

impl Dispatcher {
    fn new(
        provider: NetworkEndpoint,
        ais: HashMap<String, NetworkEndpoint>,
        location_tx: Sender<ParsedMessage>,
        interval: u64,
        location_interval: u64,
        location_anchor_interval: u64,
    ) -> Self {
        Dispatcher {
            provider,
            ais,
            location_tx,
            interval,
            location_interval,
            location_anchor_interval,
            nmea_parser: nmea_parser::NmeaParser::new(),
            last_sent: HashMap::new(),
            last_sent_location: SystemTime::now() - Duration::from_secs(location_interval),
        }
    }

    fn next_location_system_time(&self, now: &SystemTime) -> SystemTime {
        let next_instant = now.add(Duration::from_secs(self.location_interval));
        let next_instant_secs = next_instant
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap() // Since this is now plus the interval, this should always be valid
            .as_secs();
        let next_instant_secs = next_instant_secs - (next_instant_secs % self.location_interval);
        SystemTime::UNIX_EPOCH + Duration::from_secs(next_instant_secs)
    }
    fn next_location_anchor_system_time(&self, now: &SystemTime) -> SystemTime {
        let next_instant = now.add(Duration::from_secs(self.location_anchor_interval));
        let next_instant_secs = next_instant
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap() // Since this is now plus the interval, this should always be valid
            .as_secs();
        let next_instant_secs =
            next_instant_secs - (next_instant_secs % self.location_anchor_interval);
        SystemTime::UNIX_EPOCH + Duration::from_secs(next_instant_secs)
    }

    // Send AIS messages to the AIS endpoints and handle location updates.
    // When a RMC message has been received recently, we will use that for the location update.
    // Otherwise, we will use the last known location from the AIS messages.
    // The location update will be sent to the location receiver thread.
    // The location update will be sent every `location_interval` seconds when the vessel is
    // moving or every `location_anchor_interval` seconds when the vessel is not moving.
    fn work(&mut self, rmc_source: &RmcSource) -> io::Result<()> {
        const RMC_MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

        let mut fragments = Vec::new();
        let mut last_seen_rmc_message = SystemTime::UNIX_EPOCH;
        let mut prev_lat = 0.0;
        let mut prev_long = 0.0;
        let now = SystemTime::now();
        let mut next_location_ts = self.next_location_system_time(&now);
        let mut next_location_anchor_ts = self.next_location_anchor_system_time(&now);

        loop {
            log::trace!("Waiting for message from provider");
            let message = self.provider.read_to_string()?;
            log::trace!("Received message: {}", message);

            for line in message.lines() {
                log::trace!("Received line: {}", line);
                match self.nmea_parser.parse_sentence(line) {
                    Ok(parsed_message) => {
                        if parsed_message == ParsedMessage::Incomplete {
                            fragments.push(line.to_string());
                            continue;
                        }
                        log::debug!("Parsed message: {:?}", parsed_message);
                        let now = SystemTime::now();

                        if let (Some(own_vessel), lat, long) = match &parsed_message {
                            ParsedMessage::VesselDynamicData(data) => (
                                Some(
                                    *rmc_source != RmcSource::RMC
                                        && last_seen_rmc_message + RMC_MESSAGE_TIMEOUT > now
                                        && data.own_vessel,
                                ),
                                data.latitude,
                                data.longitude,
                            ),
                            ParsedMessage::VesselStaticData(_data) => (Some(false), None, None),
                            ParsedMessage::Rmc(data) => {
                                if *rmc_source != RmcSource::AIS {
                                    last_seen_rmc_message = now;
                                    (Some(true), data.latitude, data.longitude)
                                } else {
                                    (None, None, None)
                                }
                            }
                            _ => (None, None, None),
                        } {
                            fragments.push(line.to_string());
                            // Ignore messages with no position or at (0, 0) coordinates
                            if let (Some(lat), Some(long)) = (lat, long) {
                                log::trace!("Parsed position: lat: {}, long: {}", lat, long);
                                if lat != 0.0 || long != 0.0 {
                                    if self.check_last_sent(&parsed_message) {
                                        self.broadcast_ais(
                                            &parsed_message,
                                            fragments.join("").as_bytes(),
                                        )?;
                                    }
                                    if own_vessel {
                                        log::trace!(
                                            "Compare last sent location: {:?} interval {:?} anchor {:?}",
                                            now,
                                            next_location_ts,
                                            next_location_anchor_ts,
                                        );
                                        if now >= next_location_anchor_ts
                                            || (now >= next_location_ts
                                                && is_moving(lat, long, prev_lat, prev_long))
                                        {
                                            prev_lat = lat;
                                            prev_long = long;
                                            self.last_sent_location = now;
                                            self.location_tx.send(parsed_message).unwrap();
                                            next_location_ts = self.next_location_system_time(&now);
                                            next_location_anchor_ts =
                                                self.next_location_anchor_system_time(&now);
                                        }
                                    }
                                }
                            }
                            fragments.clear();
                        }
                    }
                    Err(_e) => {
                        fragments.clear();
                    }
                }
            }
        }
    }

    fn broadcast_ais(&mut self, message: &ParsedMessage, nmea_message: &[u8]) -> io::Result<()> {
        log::debug!("Broadcasting message: {:?} / {:?}", message, nmea_message);
        for (key, address) in self.ais.iter_mut() {
            send_message(&nmea_message, key, address)?;
        }
        Ok(())
    }

    fn check_last_sent(&mut self, message: &ParsedMessage) -> bool {
        match message {
            ParsedMessage::VesselDynamicData(data) => {
                let now = Instant::now();
                let elapsed = now - Duration::from_secs(self.interval);
                let last_sent = self.last_sent.entry(data.mmsi).or_insert(LastSent {
                    vessel_dynamic_data: elapsed,
                    vessel_static_data: elapsed,
                });
                let elapsed_secs = now.duration_since(last_sent.vessel_dynamic_data).as_secs();
                if elapsed_secs >= self.interval {
                    last_sent.vessel_dynamic_data = now;
                    log::debug!(
                        "Sending dynamic data for MMSI {} as we last sent it {} seconds ago",
                        data.mmsi,
                        elapsed_secs
                    );
                    return true;
                }
                log::debug!(
                    "Skipping dynamic data for MMSI {} as we last sent it {} seconds ago",
                    data.mmsi,
                    elapsed_secs
                );
            }
            ParsedMessage::VesselStaticData(data) => {
                let now = Instant::now();
                let elapsed = now - Duration::from_secs(self.interval);
                let last_sent = self.last_sent.entry(data.mmsi).or_insert(LastSent {
                    vessel_dynamic_data: elapsed,
                    vessel_static_data: elapsed,
                });
                let elapsed_secs = now.duration_since(last_sent.vessel_static_data).as_secs();
                if elapsed_secs >= self.interval {
                    last_sent.vessel_static_data = now;
                    log::debug!(
                        "Sending static data for MMSI {} as we last sent it {} seconds ago",
                        data.mmsi,
                        elapsed_secs
                    );
                    return true;
                }
                log::debug!(
                    "Skipping static data for MMSI {} as we last sent it {} seconds ago",
                    data.mmsi,
                    elapsed_secs
                );
            }
            _ => {
                log::debug!("Ignoring message: {:?}", message);
            }
        }
        return false;
    }
}

fn is_moving(lat: f64, long: f64, prev_lat: f64, prev_long: f64) -> bool {
    let lat_diff = (lat - prev_lat).abs();
    let long_diff = (long - prev_long).abs();

    lat_diff > 0.001 || long_diff > 0.001
}

fn send_message(
    nmea_message: &[u8],
    key: &String,
    address: &mut NetworkEndpoint,
) -> io::Result<()> {
    match address.protocol {
        Protocol::TCP => {
            address.tcp_stream.retain(|writer| {
                if writer.peer_addr().is_err() {
                    log::warn!("Removing disconnected TCP stream");
                    false
                } else {
                    true
                }
            });

            if address.tcp_stream.len() == 0 {
                let stream = std::net::TcpStream::connect(address.addr).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("{} ({}): {}", key, address.addr, e),
                    )
                })?;

                // Set the stream to use keepalive
                let sock_ref = socket2::SockRef::from(&stream);
                let mut ka = socket2::TcpKeepalive::new();
                ka = ka.with_time(Duration::from_secs(30));
                ka = ka.with_interval(Duration::from_secs(30));
                sock_ref.set_tcp_keepalive(&ka)?;

                log::info!("{}: Connected to {}", key, address);
                let writer = BufReaderDirectWriter::new(stream);
                address.tcp_stream.push(writer);
            }
            if let Some(tcp_stream) = address.tcp_stream.get_mut(0) {
                send_message_tcp(tcp_stream, nmea_message).map_err(|e| {
                    address.tcp_stream.clear();
                    std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("send_message tcp {} ({}): {}", key, address.addr, e),
                    )
                })?;
                log::debug!("{}: Sent message to {}", key, address);
            }
        }
        Protocol::UDP => {
            if address.udp_socket.is_none() {
                let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("{} ({}): {}", key, address.addr, e),
                    )
                })?;
                UdpSocket::connect(&socket, address.addr)?;
                log::info!("{}: Connected to {}", key, address);
                address.udp_socket = Some(socket);
            }
            if let Some(udp_socket) = address.udp_socket.as_mut() {
                send_message_udp(udp_socket, nmea_message).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("send_message udp {} ({}): {}", key, address.addr, e),
                    )
                })?;
            }
        }
        Protocol::TCPListen | Protocol::UDPListen => {}
    }
    Ok(())
}

fn get_config_dir() -> PathBuf {
    let path = if path::Path::new("/etc/ais-forwarder").exists() {
        "/etc/ais-forwarder"
    } else if path::Path::new("/usr/local/etc/ais-forwarder").exists() {
        "/usr/local/etc/ais-forwarder"
    } else {
        log::error!(
            "No /etc/ais-forwarder or /usr/local/etc/ais-forwarder config directory found and no config file argument provided"
        );
        exit(1);
    };
    let path = path::Path::new(path);
    path.to_path_buf()
}
