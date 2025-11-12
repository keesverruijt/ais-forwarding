use std::path::PathBuf;

use sled::*;

#[derive(Debug, Clone)]
pub struct Persistence {
    db: Db,
    count: usize,
}

#[allow(dead_code)]
impl Persistence {
    pub const NEXT_KEY_VALUE: [u8; 1] = [0];
    const INITIAL_KEY_VALUE: [u8; 8] = [0; 8];

    pub fn new(cache_dir: &str) -> Self {
        let database_path = PathBuf::from(cache_dir);
        if !database_path.exists() {
            std::fs::create_dir_all(&database_path).expect("Cannot create database directory");
        }

        let db: Db = sled::Config::default()
            .cache_capacity(5_000)
            .path(&database_path)
            .open()
            .expect(format!("Cannot open database {}", database_path.display()).as_str());

        let next_key = db.get(&Self::NEXT_KEY_VALUE).unwrap();
        if next_key.is_none() {
            db.insert(&Self::NEXT_KEY_VALUE, &Self::INITIAL_KEY_VALUE)
                .unwrap();
        }
        let count = db.len() - 1;

        let this = Persistence { db, count };

        log::debug!("database loaded from {}", database_path.display());

        this
    }

    pub fn store(&mut self, key: &[u8], value: &[u8]) {
        if self.db.insert(key, value).unwrap().is_none() {
            self.count += 1;
        }
    }

    pub fn iter(&self) -> sled::Iter {
        self.db.iter()
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        match self.db.get(key) {
            Ok(Some(value)) => Some(value.to_vec()),
            Ok(None) => None,
            Err(e) => {
                log::error!("Error getting value from database: {}", e);
                None
            }
        }
    }

    pub fn remove(&mut self, key: &[u8]) {
        if self.db.remove(key).unwrap().is_some() {
            self.count -= 1;
        }
    }

    pub fn flush(&self) {
        self.db.flush().unwrap();
    }

    pub fn clear(&self) {
        self.db.clear().unwrap();
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn next_key(&mut self) -> u64 {
        match self
            .db
            .update_and_fetch(&Self::NEXT_KEY_VALUE, increment)
            .unwrap()
        {
            Some(n) => iv_to_u64(n).unwrap(),
            None => 0,
        }
    }
}

fn increment(old: Option<&[u8]>) -> Option<Vec<u8>> {
    let number = match old {
        Some(bytes) => {
            let array: [u8; 8] = bytes.try_into().unwrap();
            let number = u64::from_be_bytes(array);
            number + 1
        }
        None => 1,
    };

    Some(number.to_be_bytes().to_vec())
}

fn iv_to_u64(iv: IVec) -> Option<u64> {
    if iv.len() == 8 {
        let bytes: [u8; 8] = iv.as_ref().try_into().ok()?;
        Some(u64::from_be_bytes(bytes)) // or from_le_bytes(bytes) based on your data
    } else {
        None // Return None if the length is not 8
    }
}
