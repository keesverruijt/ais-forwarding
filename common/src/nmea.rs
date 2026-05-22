use std::{
    cmp::{max, min},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct TrueWind {
    pub direction_degrees: f64, // 0-360, relative to true north
    pub speed_knots: f64,
}

/// Parse an NMEA0183 MWV sentence with True wind reference.
/// Format: $xxMWV,angle,T,speed,units,A*checksum
/// If `talker` is Some, only accept sentences with that talker ID (e.g. "CG").
/// Returns None if the sentence is not a valid true wind MWV.
pub fn parse_mwv_true(sentence: &str, talker: Option<&str>) -> Option<TrueWind> {
    let sentence = sentence.trim();
    if sentence.len() < 10 || !sentence.starts_with('$') {
        return None;
    }
    if &sentence[3..6] != "MWV" {
        return None;
    }
    if let Some(t) = talker {
        if &sentence[1..3] != t {
            return None;
        }
    }
    let data = sentence.split('*').next()?;
    let fields: Vec<&str> = data.split(',').collect();
    if fields.len() < 6 {
        return None;
    }
    if fields[2] != "T" || fields[5] != "A" {
        return None;
    }
    let angle: f64 = fields[1].parse().ok()?;
    let speed: f64 = fields[3].parse().ok()?;
    let speed_knots = match fields[4] {
        "N" => speed,
        "K" => speed / 1.852,
        "M" => speed * 1.94384,
        _ => return None,
    };
    Some(TrueWind {
        direction_degrees: angle,
        speed_knots,
    })
}

pub struct Message18 {
    own_ship: bool,
    mmsi: u32,
    speed_over_ground: Option<f64>,  // deciknots
    longitude: Option<f64>,          // degrees
    latitude: Option<f64>,           // degrees
    course_over_ground: Option<f64>, // degrees
    true_heading: Option<u16>,       // degrees
}

impl Message18 {
    pub fn new(
        own_ship: bool,
        mmsi: u32,
        speed_over_ground: Option<f64>,
        longitude: Option<f64>,
        latitude: Option<f64>,
        course_over_ground: Option<f64>,
        true_heading: Option<u16>,
    ) -> Self {
        Message18 {
            own_ship,
            mmsi,
            speed_over_ground,
            longitude,
            latitude,
            course_over_ground,
            true_heading,
        }
    }

    pub fn to_nmea(&self) -> String {
        let mut binary_str = format!("{:06b}", 18);

        binary_str += &format!("{:02b}", 0);

        binary_str += &format!("{:030b}", self.mmsi);
        binary_str += &format!("{:08b}", 0);

        let speed_over_ground = match self.speed_over_ground {
            Some(sog) => (sog * 10.0) as u16,
            None => 1023, // Not available
        };
        binary_str += &format!("{:010b}", speed_over_ground);

        binary_str += &format!("{:01b}", 0);

        let mut longitude: f64 = match self.longitude {
            Some(lon) => lon,
            None => 181.0, // Not available
        };
        longitude *= 60.0 * 10_000.0; // Longitude in 1/10_000 minutes
        let longitude = longitude as i64 as u32 & 0x0FFFFFFF;
        binary_str += &format!("{:028b}", longitude);

        let mut latitude: f64 = match self.latitude {
            Some(lat) => lat,
            None => 91.0, // Not available
        };
        latitude *= 60.0 * 10_000.0; // Latitude in 1/10_000 minutes
        let latitude = latitude as i64 as u32 & 0x07FFFFFF;
        binary_str += &format!("{:027b}", latitude);

        binary_str += &format!(
            "{:012b}",
            match self.course_over_ground {
                Some(cog) => max(min((cog * 10.0) as u16, 3599), 0),
                None => 3600, // Not available
            }
        );
        binary_str += &format!(
            "{:09b}",
            match self.true_heading {
                Some(th) => max(min(th, 359), 0),
                None => 511, // Not available
            }
        );

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            % 60;
        log::debug!("Timestamp for AIS message: {}", timestamp);
        binary_str += &format!("{:06b}", timestamp);

        binary_str += "00"; // Spare bit
        binary_str += "1"; // SOTDMA/CS flag: CS
        binary_str += "0000011"; // Slot timeout and submessage
        binary_str += "1100000000000000110"; // Communication state

        let sentence = if self.own_ship { "AIVDO" } else { "AIVDM" };
        let content = format!("{},1,1,,A,{}", sentence, to_6bit_ascii(&binary_str));
        let checksum = calculate_checksum(&content);
        format!("!{}*{:02X}", content, checksum)
    }
}

pub struct Message24 {
    own_ship: bool,
    mmsi: u32,
    name: String,     // up to 20 chars
    ship_type: u8,    // AIS ship type code
    callsign: String, // up to 7 chars
    bow: u16,         // meters
    stern: u16,       // meters
    port: u8,         // meters
    starboard: u8,    // meters
}

impl Message24 {
    pub fn new(
        own_ship: bool,
        mmsi: u32,
        name: &str,
        ship_type: u8,
        callsign: &str,
        bow: u16,
        stern: u16,
        port: u8,
        starboard: u8,
    ) -> Self {
        Message24 {
            own_ship,
            mmsi,
            name: name.to_uppercase(),
            ship_type,
            callsign: callsign.to_uppercase(),
            bow,
            stern,
            port,
            starboard,
        }
    }

    /// Generate NMEA sentences for both Part A (name) and Part B (details).
    pub fn to_nmea(&self) -> Vec<String> {
        vec![self.part_a(), self.part_b()]
    }

    fn part_a(&self) -> String {
        let mut bits = String::new();
        bits += &format!("{:06b}", 24); // Message type
        bits += &format!("{:02b}", 0); // Repeat indicator
        bits += &format!("{:030b}", self.mmsi);
        bits += &format!("{:02b}", 0); // Part number = A
        bits += &string_to_ais6_bits(&self.name, 20); // Name (120 bits)
        bits += &format!("{:08b}", 0); // Spare

        let sentence = if self.own_ship { "AIVDO" } else { "AIVDM" };
        let content = format!("{},1,1,,A,{},0", sentence, to_6bit_ascii(&bits));
        let checksum = calculate_checksum(&content);
        format!("!{}*{:02X}", content, checksum)
    }

    fn part_b(&self) -> String {
        let mut bits = String::new();
        bits += &format!("{:06b}", 24); // Message type
        bits += &format!("{:02b}", 0); // Repeat indicator
        bits += &format!("{:030b}", self.mmsi);
        bits += &format!("{:02b}", 1); // Part number = B
        bits += &format!("{:08b}", self.ship_type);
        bits += &string_to_ais6_bits("", 3); // Vendor ID (18 bits)
        bits += &format!("{:04b}", 0); // Unit model code
        bits += &format!("{:020b}", 0); // Serial number
        bits += &string_to_ais6_bits(&self.callsign, 7); // Callsign (42 bits)
        bits += &format!("{:09b}", min(self.bow as u16, 511));
        bits += &format!("{:09b}", min(self.stern as u16, 511));
        bits += &format!("{:06b}", min(self.port, 63));
        bits += &format!("{:06b}", min(self.starboard, 63));
        bits += &format!("{:04b}", 1); // Position fix type = GPS
        bits += &format!("{:02b}", 0); // Spare

        let sentence = if self.own_ship { "AIVDO" } else { "AIVDM" };
        let content = format!("{},1,1,,A,{},0", sentence, to_6bit_ascii(&bits));
        let checksum = calculate_checksum(&content);
        format!("!{}*{:02X}", content, checksum)
    }
}

/// Convert a character to AIS 6-bit encoding (ITU-R M.1371-5).
fn char_to_ais6(c: char) -> u8 {
    let c = c.to_ascii_uppercase() as u8;
    if c >= 64 && c <= 95 {
        c - 64 // '@'=0, 'A'=1, ..., 'Z'=26, ...
    } else if c >= 32 && c <= 63 {
        c // ' '=32, '!'=33, ..., '?'=63
    } else {
        0 // '@' for unknown
    }
}

/// Encode a string as AIS 6-bit binary, padded to `max_chars` with '@' (0).
fn string_to_ais6_bits(s: &str, max_chars: usize) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for i in 0..max_chars {
        let c = if i < chars.len() { chars[i] } else { '@' };
        result += &format!("{:06b}", char_to_ais6(c));
    }
    result
}

fn to_6bit_ascii(binary_str: &str) -> String {
    let pad_str = "000000".repeat((6 - (binary_str.len() % 6)) % 6);
    let binary_str = binary_str.to_string() + &pad_str;

    let mut ascii_str = String::new();
    for chunk in binary_str.chars().collect::<Vec<_>>().chunks(6) {
        let mut chunk_str = String::new();
        for c in chunk {
            chunk_str.push(*c);
        }
        let chunk_num = u8::from_str_radix(&chunk_str, 2).unwrap();
        // Algorithm from ITU-R M.1371-5 figure 3
        let ascii_char = (chunk_num + if chunk_num < 40 { 48 } else { 56 }) as char;
        ascii_str.push(ascii_char);
    }
    ascii_str
}

pub fn calculate_checksum(input: &str) -> u8 {
    let mut checksum = 0u8;
    for c in input.chars() {
        checksum ^= c as u8;
    }
    checksum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mwv_true_wind() {
        let wind = parse_mwv_true("$CGMWV,124.3,T,31.8,K,A*04", None).unwrap();
        assert!((wind.direction_degrees - 124.3).abs() < 0.01);
        assert!((wind.speed_knots - 31.8 / 1.852).abs() < 0.1);
    }

    #[test]
    fn test_parse_mwv_relative_ignored() {
        assert!(parse_mwv_true("$DFMWV,91.5,R,28.7,K,A*3A", None).is_none());
    }

    #[test]
    fn test_parse_mwv_invalid_status() {
        assert!(parse_mwv_true("$CGMWV,124.3,T,31.8,K,V*04", None).is_none());
    }

    #[test]
    fn test_parse_mwv_talker_filter() {
        assert!(parse_mwv_true("$CGMWV,124.3,T,31.8,K,A*04", Some("CG")).is_some());
        assert!(parse_mwv_true("$CGMWV,124.3,T,31.8,K,A*04", Some("DF")).is_none());
    }

    #[test]
    fn test_parse_mwv_knots() {
        let wind = parse_mwv_true("$IIMWV,270.0,T,15.0,N,A*00", None).unwrap();
        assert!((wind.speed_knots - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_message24_to_nmea() {
        let message = Message24::new(true, 244123456, "MERRIMAC", 36, "PH1234", 12, 3, 2, 2);
        let sentences = message.to_nmea();
        assert_eq!(sentences.len(), 2);
        println!("Part A: {}", sentences[0]);
        println!("Part B: {}", sentences[1]);
        assert!(sentences[0].starts_with("!AIVDO"));
        assert!(sentences[1].starts_with("!AIVDO"));

        let mut parser = nmea_parser::NmeaParser::new();

        let parsed_a = parser.parse_sentence(&sentences[0]).unwrap();
        assert_eq!(parsed_a, nmea_parser::ParsedMessage::Incomplete);

        let parsed_b = parser.parse_sentence(&sentences[1]).unwrap();
        match parsed_b {
            nmea_parser::ParsedMessage::VesselStaticData(data) => {
                assert_eq!(data.mmsi, 244123456);
                assert_eq!(data.name, Some("MERRIMAC".to_string()));
                assert_eq!(data.call_sign, Some("PH1234".to_string()));
                assert_eq!(data.dimension_to_bow, Some(12));
                assert_eq!(data.dimension_to_stern, Some(3));
                assert_eq!(data.dimension_to_port, Some(2));
                assert_eq!(data.dimension_to_starboard, Some(2));
            }
            other => panic!("Expected VesselStaticData, got {:?}", other),
        }
    }

    #[test]
    fn test_message18_to_nmea() {
        let lon = -15.0;
        let lat = -85.0;

        let message = Message18::new(
            true,
            123456789,
            Some(12.3),
            Some(lon),
            Some(lat),
            Some(89.0),
            Some(270),
        );
        let nmea = message.to_nmea();
        println!("Generated NMEA: {}", nmea);
        assert!(nmea.starts_with("!AIVDO"));

        let mut parser = nmea_parser::NmeaParser::new();
        let parsed = parser.parse_sentence(&nmea).unwrap();
        match parsed {
            nmea_parser::ParsedMessage::VesselDynamicData(ais_msg) => {
                assert_eq!(ais_msg.mmsi, 123456789);
                assert_eq!(ais_msg.sog_knots, Some(12.3));
                assert_eq!(ais_msg.longitude, Some(lon));
                assert_eq!(ais_msg.latitude, Some(lat));
                assert_eq!(ais_msg.cog, Some(89.0));
                assert_eq!(ais_msg.heading_true, Some(270.0));
            }
            _ => panic!("Expected AIS message"),
        }
    }
}
