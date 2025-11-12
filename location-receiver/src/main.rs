use ::time::OffsetDateTime;
use env_logger::Env;
use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;
use std::time::SystemTime;

use common::buffer::BufReaderDirectWriter;

fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    log::info!("location-receiver starting up");

    let db_path = Path::new("/var/db");
    std::fs::create_dir_all(&db_path).expect("Cannot create /var/db directory");

    let listener = TcpListener::bind("10.67.0.1:11328").expect("Cannot bind to port 11328");

    loop {
        let (stream, addr) = listener.accept().expect("Failed to accept connection");
        log::info!("Accepted connection from: {}", addr);
        thread::spawn(move || {
            let mut message = String::new();
            let mut reader = BufReaderDirectWriter::new(stream);
            loop {
                match reader.read_line(&mut message) {
                    Ok(0) => break, // Connection closed
                    Ok(1) => {}
                    Ok(_) => {
                        log::info!("Received message: {}", message);
                        // Process the message here

                        if process_message(&message, &db_path).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        log::error!("Error reading from stream: {}", e);
                        break;
                    }
                }
                let i = message.find('@').unwrap_or(0);
                if i == 8 {
                    let (key, _) = message.split_at(i);
                    let line = format!("{}\n", key);
                    match reader.write_all(line.as_bytes()) {
                        Ok(()) => {}
                        Err(e) => {
                            log::error!("Error writing reply to stream: {}", e);
                            break;
                        }
                    };
                }
                message.clear();
            }
        });
    }
}

fn process_message(message: &str, db_path: &Path) -> std::io::Result<()> {
    // Parse the message and handle it accordingly
    let i = message.find('$').unwrap_or(0);
    if i == 0 {
        log::error!("No MMSI/boatname in '{}'", message);
        return Ok(());
    }
    let (id, message) = message.split_at(i);
    let i = id.find('@').unwrap_or(0);
    let (_key, id) = id.split_at(i);

    let nmea_id = &message[3..6].to_lowercase();
    let new_line = format!("{}\r\n", message);
    let now: OffsetDateTime = SystemTime::now().into();
    let year = now.year();

    let path = db_path.join(format!("{}_{}_{}.db", id, year, nmea_id));

    // Prevent duplicate entries. This means reading through a ~200k file every
    // ten minutes or so, which doesn't seem like it needs optimizing.
    match std::fs::OpenOptions::new().read(true).open(&path) {
        Ok(file) => {
            let file = BufReaderDirectWriter::new(file);
            for line in file.lines() {
                if let Ok(line) = line
                    && line == message
                {
                    log::debug!("Duplicate entry received: {}", message);
                    return Ok(());
                }
            }
        }
        Err(_) => {}
    };

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("Cannot open database file");
    if let Err(e) = file.write_all(new_line.as_bytes()) {
        log::error!("Error writing to {}: {}", path.display(), e);
        return Err(e);
    }
    return Ok(());
}
