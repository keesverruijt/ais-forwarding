use chrono::{DateTime, TimeDelta, Utc};
/// (C) 2025 by Kees Verruijt, Harlingen, Netherlands
use nmea_parser::ParsedMessage;
use std::collections::HashMap;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::NetworkEndpoint;
use crate::cache::Persistence;

pub fn work_thread(
    rx: std::sync::mpsc::Receiver<ParsedMessage>,
    location: HashMap<String, NetworkEndpoint>,
    mmsi: u32,
    cache_dir: &str,
) {
    let persistence = Persistence::new(cache_dir);

    let _ = Location::new(location, persistence, mmsi).location_loop(&rx);
}

struct Location {
    location: HashMap<String, NetworkEndpoint>,
    persistence: Persistence,
    mmsi: u32,
    prev_latitude: Option<f64>,
    prev_longitude: Option<f64>,
    doubtful_latitude: Option<f64>,
    doubtful_longitude: Option<f64>,
    resend_timeout: DateTime<Utc>,
}

const RESEND_TIMEOUT: Duration = Duration::from_secs(360);

impl Location {
    fn new(
        location: HashMap<String, NetworkEndpoint>,
        persistence: Persistence,
        mmsi: u32,
    ) -> Self {
        let now = chrono::Utc::now();

        Self {
            location,
            persistence,
            mmsi,
            prev_latitude: None,
            prev_longitude: None,
            doubtful_latitude: None,
            doubtful_longitude: None,
            resend_timeout: now + RESEND_TIMEOUT,
        }
    }

    fn location_loop(&mut self, rx: &Receiver<ParsedMessage>) -> io::Result<()> {
        const MESSAGE_TIMEOUT: Duration = Duration::from_secs(360);

        log::info!(
            "Starting location loop with {} endpoints",
            self.location.len()
        );
        self.resend_messages();

        loop {
            match rx.recv_timeout(MESSAGE_TIMEOUT) {
                Ok(message) => {
                    if self.resend_timeout < chrono::Utc::now() {
                        self.resend_messages();
                    }
                    log::debug!("Received message: {:?}", message);
                    self.parse_message(&message);
                }
                Err(e) => match e {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {
                        self.resend_messages();
                        continue;
                    }
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        log::error!("Receiver disconnected");
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "Receiver disconnected",
                        ));
                    }
                },
            }
        }
    }

    fn resend_messages(&mut self) {
        let resend_count = self.persistence.count();
        if resend_count == 0 {
            log::info!("No messages to resend from persistence");
            return;
        }
        self.resend_timeout = chrono::Utc::now() + RESEND_TIMEOUT;

        log::info!("Resending {} messages from persistence", resend_count);
        let mut failing_locations = HashMap::new();
        for item in self.persistence.iter() {
            match item {
                Ok((key, value)) => {
                    let key = &key.to_vec();
                    let value = &value.to_vec();
                    let skey = String::from_utf8_lossy(&key);
                    let svalue = String::from_utf8_lossy(&value);

                    let old_location = skey.split("@").next().unwrap();
                    if !failing_locations.contains_key(old_location) {
                        log::debug!("Resending message: {}: {}", skey, svalue);
                        for (location, address) in self.location.iter_mut() {
                            if address.send_message(value, &location).is_ok() {
                                self.persistence.remove(key);
                                self.persistence.flush();
                                log::info!("Finally sent message {}", svalue);
                            } else {
                                failing_locations.insert(location.to_owned(), 0);
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
                        return false;
                    }
                } else {
                    log::warn!("Invalid position: latitude change is too big");
                    return false;
                }
            }
        }
        if let Some(prev_longitude) = self.prev_longitude {
            if (longitude - prev_longitude).abs() >= 2.00 {
                if let Some(doubtful_longitude) = self.doubtful_longitude {
                    if (longitude - doubtful_longitude).abs() >= 2.00 {
                        log::warn!("Doubtful position: longitude change is too big");
                        return false;
                    }
                } else {
                    log::warn!("Invalid position: longitude change is too big");
                    return false;
                }
            }
        }

        true
    }

    fn parse_message(&mut self, message: &ParsedMessage) {
        let now = chrono::Utc::now();
        const TIME_FORMAT: &str = "%H%M%S";
        const DATE_FORMAT: &str = "%d%m%y";

        let nmea_message = match message {
            ParsedMessage::VesselDynamicData(message) => {
                if !self.validate_position(message.latitude, message.longitude) {
                    // If the same "weird" position is received a second time, we assume this
                    // is the new ships position.
                    self.doubtful_latitude = message.latitude;
                    self.doubtful_longitude = message.longitude;
                    return;
                }
                self.prev_latitude = message.latitude;
                self.prev_longitude = message.longitude;
                self.doubtful_latitude = None;
                self.doubtful_longitude = None;
                format!(
                    "{}$GNRMC,{},A,{},{},{},{},{},,,A\r\n",
                    message.mmsi,
                    now.format(TIME_FORMAT),
                    Self::format_lat_long(message.latitude, true),
                    Self::format_lat_long(message.longitude, false),
                    "", // Speed over ground,
                    "", // Course over ground,
                    now.format(DATE_FORMAT),
                )
            }
            ParsedMessage::Rmc(message) => {
                if !self.validate_position(message.latitude, message.longitude) {
                    // If the same "weird" position is received a second time, we assume this
                    // is the new ships position.
                    self.doubtful_latitude = message.latitude;
                    self.doubtful_longitude = message.longitude;
                    return;
                }
                self.prev_latitude = message.latitude;
                self.prev_longitude = message.longitude;
                self.doubtful_latitude = None;
                self.doubtful_longitude = None;
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
                    "{}$GNRMC,{},A,{},{},{},{},{},,,A\r\n",
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
                log::warn!("Unsupported message type: {:?}", message);
                return;
            }
        };

        let nmea_bytes = nmea_message.as_bytes();
        for (location, address) in self.location.iter_mut() {
            let db_key = format!("{}@{}", location, now);
            if !address.is_connected() {
                log::debug!("Storing message: {}: {}", location, nmea_message);
                self.persistence.store(db_key.as_bytes(), nmea_bytes);
                self.persistence.flush();
            } else {
                log::debug!("Sending message: {}: {}", location, nmea_message);
                if let Err(e) = address.send_message(&nmea_bytes, location) {
                    log::error!("Error sending location message to {}: {}", location, e);
                    self.persistence.store(db_key.as_bytes(), nmea_bytes);
                    self.persistence.flush();
                }
            }
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
