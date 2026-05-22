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
/// Returns None if the sentence is not a valid true wind MWV.
pub fn parse_mwv_true(sentence: &str) -> Option<TrueWind> {
    let sentence = sentence.trim();
    if sentence.len() < 10 || !sentence.starts_with('$') {
        return None;
    }
    if &sentence[3..6] != "MWV" {
        return None;
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
        log::info!("Timestamp for AIS message: {}", timestamp);
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

fn calculate_checksum(input: &str) -> u8 {
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
        let wind = parse_mwv_true("$CGMWV,124.3,T,31.8,K,A*04").unwrap();
        assert!((wind.direction_degrees - 124.3).abs() < 0.01);
        assert!((wind.speed_knots - 31.8 / 1.852).abs() < 0.1);
    }

    #[test]
    fn test_parse_mwv_relative_ignored() {
        assert!(parse_mwv_true("$DFMWV,91.5,R,28.7,K,A*3A").is_none());
    }

    #[test]
    fn test_parse_mwv_invalid_status() {
        assert!(parse_mwv_true("$CGMWV,124.3,T,31.8,K,V*04").is_none());
    }

    #[test]
    fn test_parse_mwv_knots() {
        let wind = parse_mwv_true("$IIMWV,270.0,T,15.0,N,A*00").unwrap();
        assert!((wind.speed_knots - 15.0).abs() < 0.01);
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
