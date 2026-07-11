/// Shared, in-memory view of what the dispatcher is seeing, exposed read-only
/// over HTTP for the Map and Status pages. The dispatcher thread writes to it;
/// the web server thread reads snapshots. Everything is behind a single Mutex
/// held only for the brief duration of an update or a snapshot.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nmea_parser::ParsedMessage;
use nmea_parser::ais::{AisClass, ShipType};
use serde::Serialize;

/// Vessels older than this are dropped from the registry and not served.
const VESSEL_TTL: Duration = Duration::from_secs(1800);

pub type Shared = Arc<Mutex<AisState>>;

#[derive(Clone)]
struct Vessel {
    mmsi: u32,
    name: Option<String>,
    callsign: Option<String>,
    ship_type: u8,
    ship_type_text: String,
    class: &'static str,
    lat: Option<f64>,
    lon: Option<f64>,
    sog: Option<f64>,
    cog: Option<f64>,
    heading: Option<f64>,
    nav_status: Option<String>,
    last_seen: SystemTime,
}

#[derive(Default)]
struct Stats {
    vdm_messages: u64,
    non_vdm_messages: u64,
    crc_errors: u64,
    duplicates: u64,
    channel_a: u64,
    channel_b: u64,
    bytes_in: u64,
    bytes_out: u64,
    invalid: u64,
    /// Index 1..=27 used; index 0 unused.
    msg_type: [u64; 28],
    per_dest: HashMap<String, u64>,
    in_roll: RollingBytes,
    out_roll: RollingBytes,
}

/// A 60-second sliding window of byte counts, one bucket per second, for
/// computing a recent bytes/second rate instead of a lifetime average.
struct RollingBytes {
    buckets: [u64; 60],
    last_sec: u64,
}

impl Default for RollingBytes {
    fn default() -> Self {
        RollingBytes {
            buckets: [0; 60],
            last_sec: 0,
        }
    }
}

impl RollingBytes {
    /// Zero out the buckets for every whole second that has elapsed since the
    /// last update, so stale counts don't linger in the window.
    fn advance(&mut self, now_sec: u64) {
        if now_sec <= self.last_sec {
            return;
        }
        let gap = (now_sec - self.last_sec).min(60);
        for i in 1..=gap {
            self.buckets[((self.last_sec + i) % 60) as usize] = 0;
        }
        self.last_sec = now_sec;
    }

    fn add(&mut self, now_sec: u64, bytes: u64) {
        self.advance(now_sec);
        self.buckets[(now_sec % 60) as usize] += bytes;
    }

    /// Bytes per second averaged over the trailing 60-second window.
    fn per_sec(&mut self, now_sec: u64) -> f64 {
        self.advance(now_sec);
        let sum: u64 = self.buckets.iter().sum();
        sum as f64 / 60.0
    }
}

pub struct AisState {
    started: SystemTime,
    station_lat: Option<f64>,
    station_lon: Option<f64>,
    title: String,
    own_mmsi: u32,
    vessels: HashMap<u32, Vessel>,
    stats: Stats,
    /// When we last saw any input line; drives the no-data watchdog.
    last_input: SystemTime,
}

impl AisState {
    pub fn new(
        title: String,
        own_mmsi: u32,
        station_lat: Option<f64>,
        station_lon: Option<f64>,
    ) -> Shared {
        Arc::new(Mutex::new(AisState {
            started: SystemTime::now(),
            station_lat,
            station_lon,
            title,
            own_mmsi,
            vessels: HashMap::new(),
            stats: Stats::default(),
            last_input: SystemTime::now(),
        }))
    }

    /// Seconds since the last input line arrived. Used by the watchdog to exit
    /// (for a supervisor to restart) when the feed goes silent.
    pub fn idle_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(self.last_input)
            .unwrap_or_default()
            .as_secs()
    }

    // ---- writes from the dispatcher ------------------------------------

    /// Account for one raw input line: byte count, VDM vs non-VDM, per-type and
    /// per-channel counters, and CRC errors. Call this for every line read from
    /// the provider, before parsing.
    pub fn record_input_line(&mut self, line: &str) {
        let bytes = line.len() as u64 + 1; // + newline
        self.stats.bytes_in += bytes;
        self.stats.in_roll.add(now_sec(), bytes);
        self.last_input = SystemTime::now();

        // CRC check applies to any checksummed sentence ($.. or !..).
        if let Some(false) = common::nmea::verify_checksum(line) {
            self.stats.crc_errors += 1;
        }

        match common::nmea::ais_message_type(line) {
            Some((msg_type, channel)) => {
                self.stats.vdm_messages += 1;
                match channel {
                    'A' => self.stats.channel_a += 1,
                    'B' => self.stats.channel_b += 1,
                    _ => {}
                }
                if (1..=27).contains(&msg_type) {
                    self.stats.msg_type[msg_type as usize] += 1;
                } else {
                    self.stats.invalid += 1;
                }
            }
            None => {
                // Continuation fragments of a multi-part AIVDM start with '!'
                // too; only count genuinely non-AIS sentences here.
                if line.trim_start().starts_with('$') {
                    self.stats.non_vdm_messages += 1;
                }
            }
        }
    }

    /// A message that was suppressed by the per-MMSI downsampling interval.
    pub fn record_duplicate(&mut self) {
        self.stats.duplicates += 1;
    }

    /// Bytes forwarded to an output endpoint.
    pub fn record_output(&mut self, dest: &str, bytes: usize) {
        self.stats.bytes_out += bytes as u64;
        self.stats.out_roll.add(now_sec(), bytes as u64);
        *self.stats.per_dest.entry(dest.to_string()).or_insert(0) += 1;
    }

    /// Fold a parsed vessel message into the live registry.
    pub fn update_vessel(&mut self, msg: &ParsedMessage) {
        match msg {
            ParsedMessage::VesselDynamicData(d) => {
                let v = self.vessel_entry(d.mmsi, class_str(d.ais_type));
                if d.latitude.is_some() {
                    v.lat = d.latitude;
                }
                if d.longitude.is_some() {
                    v.lon = d.longitude;
                }
                v.sog = d.sog_knots;
                v.cog = d.cog;
                v.heading = d.heading_true;
                v.nav_status = Some(format!("{}", d.nav_status));
                v.class = class_str(d.ais_type);
                v.last_seen = SystemTime::now();
            }
            ParsedMessage::VesselStaticData(s) => {
                let v = self.vessel_entry(s.mmsi, class_str(s.ais_type));
                if s.name.is_some() {
                    v.name = s.name.clone();
                }
                if s.call_sign.is_some() {
                    v.callsign = s.call_sign.clone();
                }
                if s.ship_type != ShipType::NotAvailable {
                    v.ship_type = s.ship_type.to_value();
                    v.ship_type_text = format!("{}", s.ship_type);
                }
                v.last_seen = SystemTime::now();
            }
            _ => {}
        }
    }

    fn vessel_entry(&mut self, mmsi: u32, class: &'static str) -> &mut Vessel {
        self.vessels.entry(mmsi).or_insert_with(|| Vessel {
            mmsi,
            name: None,
            callsign: None,
            ship_type: 0,
            ship_type_text: String::new(),
            class,
            lat: None,
            lon: None,
            sog: None,
            cog: None,
            heading: None,
            nav_status: None,
            last_seen: SystemTime::now(),
        })
    }

    // ---- reads for the web server --------------------------------------

    pub fn vessels_json(&mut self) -> serde_json::Value {
        let now = SystemTime::now();
        // Drop stale vessels while we're here.
        self.vessels
            .retain(|_, v| now.duration_since(v.last_seen).unwrap_or_default() < VESSEL_TTL);

        let vessels: Vec<VesselJson> = self
            .vessels
            .values()
            .filter(|v| v.lat.is_some() && v.lon.is_some())
            .map(|v| VesselJson {
                mmsi: v.mmsi,
                name: v.name.clone(),
                callsign: v.callsign.clone(),
                ship_type: v.ship_type,
                ship_type_text: v.ship_type_text.clone(),
                class: v.class,
                lat: v.lat,
                lon: v.lon,
                sog: v.sog,
                cog: v.cog,
                heading: v.heading,
                nav_status: v.nav_status.clone(),
                age: now
                    .duration_since(v.last_seen)
                    .unwrap_or_default()
                    .as_secs(),
                own: v.mmsi == self.own_mmsi,
            })
            .collect();

        serde_json::json!({
            "generated": unix_secs(now),
            "station": {
                "lat": self.station_lat,
                "lon": self.station_lon,
                "label": self.title,
            },
            "total": vessels.len(),
            "vessels": vessels,
        })
    }

    pub fn stats_json(&mut self) -> serde_json::Value {
        let now = SystemTime::now();
        let uptime = now.duration_since(self.started).unwrap_or_default().as_secs();
        // Bytes/second over the trailing 60-second window.
        let now_s = now_sec();
        let in_bw = self.stats.in_roll.per_sec(now_s);
        let out_bw = self.stats.out_roll.per_sec(now_s);
        let s = &self.stats;

        let by_type: Vec<u64> = (1..=27).map(|i| s.msg_type[i]).collect();
        let mut per_dest: Vec<serde_json::Value> = s
            .per_dest
            .iter()
            .map(|(k, v)| serde_json::json!({ "dest": k, "count": v }))
            .collect();
        per_dest.sort_by(|a, b| a["dest"].as_str().cmp(&b["dest"].as_str()));

        let out_ratio = if s.vdm_messages > 0 {
            let out_total: u64 = s.per_dest.values().sum();
            out_total as f64 / s.vdm_messages as f64
        } else {
            0.0
        };

        serde_json::json!({
            "title": self.title,
            "uptime": uptime,
            "input": {
                "vdm_messages": s.vdm_messages,
                "non_vdm_messages": s.non_vdm_messages,
                "crc_errors": s.crc_errors,
                "duplicates": s.duplicates,
                "channel_a": s.channel_a,
                "channel_b": s.channel_b,
                "bytes": s.bytes_in,
                "bandwidth": in_bw,
            },
            "output": {
                "in_out_ratio": out_ratio,
                "bytes": s.bytes_out,
                "bandwidth": out_bw,
                "per_dest": per_dest,
            },
            "by_message_type": by_type,
            "invalid": s.invalid,
            "vessels_tracked": self.vessels.len(),
        })
    }

}

#[derive(Serialize)]
struct VesselJson {
    mmsi: u32,
    name: Option<String>,
    callsign: Option<String>,
    ship_type: u8,
    ship_type_text: String,
    class: &'static str,
    lat: Option<f64>,
    lon: Option<f64>,
    sog: Option<f64>,
    cog: Option<f64>,
    heading: Option<f64>,
    nav_status: Option<String>,
    age: u64,
    own: bool,
}

fn class_str(c: AisClass) -> &'static str {
    match c {
        AisClass::ClassA => "A",
        AisClass::ClassB => "B",
        AisClass::Unknown => "?",
    }
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn now_sec() -> u64 {
    unix_secs(SystemTime::now())
}
