use clap::Parser;
use config::Config;
use env_logger::Env;
use nmea_parser::ParsedMessage;
use std::collections::HashMap;
use std::ops::Add;
use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc::Sender;
use std::thread::Builder;
use std::time::{Duration, Instant, SystemTime};
use std::{io, path};

use common::NetworkEndpoint;
use common::nmea::{Message18, TrueWind, parse_mwv_true};

pub struct LocationMessage {
    pub parsed: ParsedMessage,
    pub true_wind: Option<TrueWind>,
}

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
    own_ship_mmsi: u32,
    provider: NetworkEndpoint,
    ais: HashMap<String, NetworkEndpoint>,
    location_tx: Sender<LocationMessage>,
    interval: u64,
    location_interval: u64,
    location_anchor_interval: u64,
    nmea_parser: nmea_parser::NmeaParser,
    last_sent: HashMap<u32, LastSent>,
    last_sent_location: SystemTime,
    prev_latitude: Option<f64>,
    prev_longitude: Option<f64>,
    doubtful_latitude: Option<f64>,
    doubtful_longitude: Option<f64>,
    latest_true_wind: Option<TrueWind>,
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
    let own_ship_info = mmsi;

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
    let location_retry_interval = match general
        .get("location_retry_interval")
        .map(|v| v.parse::<u64>())
    {
        None => 600,
        Some(Ok(interval)) => interval,
        Some(Err(e)) => {
            log::error!("Invalid location_retry_interval in config.ini: {}", e);
            exit(1);
        }
    };

    let (tx, rx) = std::sync::mpsc::channel::<LocationMessage>();
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
            location::work_thread(
                rx,
                location,
                mmsi,
                cli.cache_dir.as_str(),
                location_retry_interval,
            );
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
            own_ship_info,
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
        own_ship_mmsi: u32,
        provider: NetworkEndpoint,
        ais: HashMap<String, NetworkEndpoint>,
        location_tx: Sender<LocationMessage>,
        interval: u64,
        location_interval: u64,
        location_anchor_interval: u64,
    ) -> Self {
        Dispatcher {
            own_ship_mmsi,
            provider,
            ais,
            location_tx,
            interval,
            location_interval,
            location_anchor_interval,
            nmea_parser: nmea_parser::NmeaParser::new(),
            last_sent: HashMap::new(),
            last_sent_location: SystemTime::now() - Duration::from_secs(location_interval),
            prev_latitude: None,
            prev_longitude: None,
            doubtful_latitude: None,
            doubtful_longitude: None,
            latest_true_wind: None,
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
        const OWN_SHIP_AIS_TIMEOUT: Duration = Duration::from_secs(60);

        let mut fragments = Vec::new();
        let mut last_seen_rmc_message = SystemTime::UNIX_EPOCH;
        let now = SystemTime::now();
        let mut last_own_ship_ais_sent = now;
        let mut next_location_ts = self.next_location_system_time(&now);
        let mut next_location_anchor_ts = self.next_location_anchor_system_time(&now);
        let mut next_dump_ts = now.add(Duration::from_secs(0));
        let mut ais_counter: u64 = 0;

        log::info!(
            "Starting dispatcher, provider {} with {} AIS endpoints",
            self.provider,
            self.ais.len(),
        );

        loop {
            let now = SystemTime::now();

            if now >= next_dump_ts {
                next_dump_ts = now.add(Duration::from_secs(60));
                let now = Instant::now();
                let goalpost = now - Duration::from_secs(120);
                log::info!(
                    "Dumping AIS messages to the cache; sent {} messages",
                    ais_counter
                );
                ais_counter = 0;
                for (mmsi, ts) in self.last_sent.iter() {
                    if ts.vessel_dynamic_data >= goalpost || ts.vessel_static_data >= goalpost {
                        log::info!(
                            "MMSI {}: static {}s, dynamic {}s",
                            mmsi,
                            (now - ts.vessel_static_data).as_secs(),
                            (now - ts.vessel_dynamic_data).as_secs()
                        );
                    }
                }
            }

            let message = self.provider.read_to_string()?;

            for line in message.lines() {
                if let Some(wind) = parse_mwv_true(line) {
                    self.latest_true_wind = Some(wind);
                    continue;
                }

                match self.nmea_parser.parse_sentence(line) {
                    Ok(parsed_message) => {
                        if parsed_message == ParsedMessage::Incomplete {
                            fragments.push(line.to_string());
                            continue;
                        }

                        if let (Some(own_vessel), lat, long) = match &parsed_message {
                            ParsedMessage::VesselDynamicData(data) => {
                                if data.own_vessel || data.mmsi == self.own_ship_mmsi {
                                    last_own_ship_ais_sent = now;
                                }
                                (
                                    Some(
                                        *rmc_source != RmcSource::RMC
                                            && last_seen_rmc_message + RMC_MESSAGE_TIMEOUT < now
                                            && (data.own_vessel || data.mmsi == self.own_ship_mmsi),
                                    ),
                                    data.latitude,
                                    data.longitude,
                                )
                            }
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
                            fragments.push(line.to_string()); // Add the last fragment to the make the message complete

                            if self.check_last_sent(&parsed_message) {
                                ais_counter += 1;
                                self.broadcast_ais(&parsed_message, fragments.join("").as_bytes())?;
                            }

                            if own_vessel && self.validate_position(lat, long) {
                                if last_own_ship_ais_sent + OWN_SHIP_AIS_TIMEOUT < now
                                    && let ParsedMessage::Rmc(data) = &parsed_message
                                {
                                    last_own_ship_ais_sent = now;
                                    ais_counter += 1;
                                    let dynamic_update_message = Message18::new(
                                        true,
                                        self.own_ship_mmsi,
                                        data.sog_knots,
                                        long,
                                        lat,
                                        data.bearing,
                                        None,
                                    );
                                    let nmea_message = dynamic_update_message.to_nmea();
                                    self.broadcast_ais(&parsed_message, nmea_message.as_bytes())?;
                                }

                                log::trace!(
                                    "Compare last sent location: {:?} interval {:?} anchor {:?}",
                                    now,
                                    next_location_ts,
                                    next_location_anchor_ts,
                                );
                                if now >= next_location_anchor_ts
                                    || (now >= next_location_ts
                                        && is_moving(
                                            lat.unwrap(),
                                            long.unwrap(),
                                            self.prev_latitude,
                                            self.prev_longitude,
                                        ))
                                {
                                    self.prev_latitude = lat;
                                    self.prev_longitude = long;
                                    self.last_sent_location = now;
                                    log::debug!("Forwarding location: {:?}", parsed_message);
                                    self.location_tx
                                        .send(LocationMessage {
                                            parsed: parsed_message,
                                            true_wind: self.latest_true_wind.clone(),
                                        })
                                        .unwrap();
                                    next_location_ts = self.next_location_system_time(&now);
                                    next_location_anchor_ts =
                                        self.next_location_anchor_system_time(&now);
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
        log::trace!("Broadcasting message: {:?} / {:?}", message, nmea_message);
        for (key, address) in self.ais.iter_mut() {
            address.send_message(&nmea_message, key)?;
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
                    log::trace!(
                        "Forwarding dynamic data for MMSI {} as we last sent it {} seconds ago",
                        data.mmsi,
                        elapsed_secs
                    );
                    return true;
                }
                log::trace!(
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
                    log::trace!(
                        "Forwarding static data for MMSI {} as we last sent it {} seconds ago",
                        data.mmsi,
                        elapsed_secs
                    );
                    return true;
                }
                log::trace!(
                    "Skipping static data for MMSI {} as we last sent it {} seconds ago",
                    data.mmsi,
                    elapsed_secs
                );
            }
            _ => {}
        }
        return false;
    }

    fn validate_position(&mut self, latitude: Option<f64>, longitude: Option<f64>) -> bool {
        if latitude.is_none() || longitude.is_none() {
            log::warn!("Invalid position: latitude or longitude is None");
            return false;
        }
        let latitude = latitude.unwrap();
        let longitude = longitude.unwrap();
        let latitude_abs = latitude.abs();
        let longitude_abs = longitude.abs();
        if latitude_abs > 90.0 || longitude_abs > 180.0 {
            log::warn!("Invalid position: latitude or longitude out of range");
            return false;
        }
        if latitude_abs < 0.01 || longitude_abs < 0.01 {
            log::warn!("Invalid position: latitude and longitude are too close to zero");
            return false;
        }
        if let Some(prev_latitude) = self.prev_latitude {
            if (latitude - prev_latitude).abs() >= 2.00 {
                if let Some(doubtful_latitude) = self.doubtful_latitude {
                    if (latitude - doubtful_latitude).abs() >= 2.00 {
                        log::warn!("Doubtful position: latitude change is too big");
                        self.doubtful_latitude = Some(latitude);
                        return false;
                    }
                } else {
                    log::warn!("Invalid position: latitude change is too big");
                    self.doubtful_latitude = Some(latitude);
                    return false;
                }
            }
        }
        if let Some(prev_longitude) = self.prev_longitude {
            if (longitude - prev_longitude).abs() >= 2.00 {
                if let Some(doubtful_longitude) = self.doubtful_longitude {
                    if (longitude - doubtful_longitude).abs() >= 2.00 {
                        log::warn!("Doubtful position: longitude change is too big");
                        self.doubtful_longitude = Some(longitude);
                        return false;
                    }
                } else {
                    log::warn!("Invalid position: longitude change is too big");
                    self.doubtful_longitude = Some(longitude);
                    return false;
                }
            }
        }

        true
    }
}

fn is_moving(lat: f64, long: f64, prev_lat: Option<f64>, prev_long: Option<f64>) -> bool {
    if let Some(prev_lat) = prev_lat
        && let Some(prev_long) = prev_long
    {
        let lat_diff = (lat - prev_lat).abs();
        let long_diff = (long - prev_long).abs();

        lat_diff > 0.001 || long_diff > 0.001
    } else {
        true
    }
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
