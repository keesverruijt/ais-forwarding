use chrono::{DateTime, TimeDelta, Utc};
/// (C) 2025 by Kees Verruijt, Harlingen, Netherlands
use nmea_parser::ParsedMessage;
use std::collections::HashMap;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::LocationMessage;
use crate::NetworkEndpoint;
use crate::cache::Persistence;

pub fn work_thread(
    rx: std::sync::mpsc::Receiver<LocationMessage>,
    location: HashMap<String, NetworkEndpoint>,
    mmsi: u32,
    cache_dir: &str,
    retry_timeout: u64,
) {
    let persistence = Persistence::new(cache_dir);

    let _ = Location::new(location, persistence, mmsi, retry_timeout).location_loop(&rx);
}

struct Location {
    location: HashMap<String, NetworkEndpoint>,
    persistence: Persistence,
    mmsi: u32,
    resend_timeout: DateTime<Utc>,
    retry_duration: Duration,
}

impl Location {
    fn new(
        location: HashMap<String, NetworkEndpoint>,
        persistence: Persistence,
        mmsi: u32,
        retry_timeout: u64,
    ) -> Self {
        let now = chrono::Utc::now();
        let retry_duration = Duration::from_secs(retry_timeout);
        log::debug!(
            "Location forwarder created with retry timeout of {} seconds",
            retry_timeout
        );

        Self {
            location,
            persistence,
            mmsi,
            resend_timeout: now,
            retry_duration,
        }
    }

    fn location_loop(&mut self, rx: &Receiver<LocationMessage>) -> io::Result<()> {
        const MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

        log::info!(
            "Starting location loop with {} endpoints",
            self.location.len()
        );

        loop {
            self.resend_messages();
            match rx.recv_timeout(MESSAGE_TIMEOUT) {
                Ok(location_message) => {
                    log::debug!("Received message: {:?}", location_message.parsed);
                    self.parse_message(&location_message);
                }
                Err(e) => match e {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {}
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        log::error!("Receiver disconnected");
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "Receiver disconnected",
                        ));
                    }
                },
            };
        }
    }

    fn resend_messages(&mut self) {
        let resend_count = self.persistence.count();
        log::debug!("{} resend from persistence", resend_count);
        if resend_count == 0 {
            return;
        }
        let now = chrono::Utc::now();
        if self.resend_timeout > now {
            log::debug!("Resend timeout not reached yet: {:?}", self.resend_timeout);

            return;
        }
        self.resend_timeout = now + self.retry_duration;

        let mut failing_locations = HashMap::new();
        for item in self.persistence.iter() {
            match item {
                Ok((key, value)) => {
                    let key = &key.to_vec();
                    let value = &value.to_vec();
                    let skey = String::from_utf8_lossy(&key);
                    let svalue = String::from_utf8_lossy(&value);

                    if *key == Persistence::NEXT_KEY_VALUE {
                        continue;
                    }

                    let old_location = skey.split("@").next().unwrap();
                    if !failing_locations.contains_key(old_location) {
                        log::debug!(
                            "Resending message: {}: {}",
                            skey,
                            &svalue[0..svalue.len() - 2]
                        );
                        for (location, address) in self.location.iter_mut() {
                            match address.send_message(value, &location) {
                                Ok(()) => match address.read_to_string() {
                                    Ok(reply) => {
                                        log::debug!(
                                            "Received confirmation of receipt of {}",
                                            reply
                                        );
                                        let reply = reply.trim_ascii_end();
                                        let db_key = format!("{}@{}", location, reply);
                                        self.persistence.remove(&db_key.into_bytes());
                                        self.persistence.flush();
                                        log::info!("Sent message {}", &svalue[0..svalue.len() - 2]);
                                    }
                                    Err(e) => {
                                        log::warn!("{}", e);
                                        failing_locations.insert(location.to_owned(), 0);
                                    }
                                },
                                Err(e) => {
                                    log::warn!("{}", e);
                                    failing_locations.insert(location.to_owned(), 0);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::error!("Error reading from database: {}", e);
                }
            }
        }
    }

    fn parse_message(&mut self, location_message: &LocationMessage) {
        let now = chrono::Utc::now();
        const TIME_FORMAT: &str = "%H%M%S";
        const DATE_FORMAT: &str = "%d%m%y";

        let unique = self.persistence.next_key();

        let mut nmea_message = match &location_message.parsed {
            ParsedMessage::VesselDynamicData(message) => {
                format!(
                    "{:08x}@{}$GNRMC,{},A,{},{},{},{},{},,,A\r\n",
                    unique,
                    message.mmsi,
                    now.format(TIME_FORMAT),
                    Self::format_lat_long(message.latitude, true),
                    Self::format_lat_long(message.longitude, false),
                    Self::format_option(message.sog_knots),
                    Self::format_option(message.cog),
                    now.format(DATE_FORMAT),
                )
            }
            ParsedMessage::Rmc(message) => {
                let ts = if let Some(ts) = message.timestamp {
                    if (now - ts).abs() > TimeDelta::minutes(60) {
                        log::error!("Message has weird timestamp: {:?}, using {}", message, now);
                        now
                    } else {
                        ts
                    }
                } else {
                    now
                };
                format!(
                    "{:08x}@{}$GNRMC,{},A,{},{},{},{},{},,,A\r\n",
                    unique,
                    self.mmsi,
                    ts.format(TIME_FORMAT),
                    Self::format_lat_long(message.latitude, true),
                    Self::format_lat_long(message.longitude, false),
                    Self::format_option(message.sog_knots),
                    Self::format_option(message.bearing),
                    ts.format(DATE_FORMAT),
                )
            }
            _ => {
                log::warn!("Unsupported message type: {:?}", location_message.parsed);
                return;
            }
        };

        if let Some(wind) = &location_message.true_wind {
            nmea_message += &format!(
                "$IIMWV,{:.1},T,{:.1},N,A\r\n",
                wind.direction_degrees, wind.speed_knots
            );
        }

        let nmea_bytes = nmea_message.as_bytes();
        for (location, _) in self.location.iter_mut() {
            let db_key = format!("{}@{:08x}", location, unique);
            self.persistence.store(db_key.as_bytes(), nmea_bytes);
            self.persistence.flush();
            log::debug!("Stored message: {}: {}", location, nmea_message);
            log::debug!(
                "There are now {} messages waiting for delivery",
                self.persistence.count()
            );
        }
    }

    fn format_option(value: Option<f64>) -> String {
        match value {
            Some(value) => format!("{:.1}", value),
            None => "".to_string(),
        }
    }

    fn format_lat_long(latlon: Option<f64>, is_lat: bool) -> String {
        match latlon {
            Some(value) => {
                let hemisphere = if is_lat {
                    if value >= 0.0 { "N" } else { "S" }
                } else {
                    if value >= 0.0 { "E" } else { "W" }
                };
                let abs_value = value.abs();
                let degrees = abs_value.trunc();
                let minutes = (abs_value - degrees) * 60.0;
                format!("{:.5},{}", degrees * 100.0 + minutes, hemisphere)
            }
            None => ",".to_string(),
        }
    }
}
