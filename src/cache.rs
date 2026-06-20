use std::fs::{self, File};
use std::path::{Path, PathBuf};
use serde::{de::DeserializeOwned, Serialize};
use chrono::Utc;

const CACHE_DIR: &str = ".cache";

pub const TTL_YAHOO_SECS: i64 = 3_600;    // 1 hour  — price data goes stale quickly
pub const TTL_FRED_SECS: i64 = 86_400;    // 24 hours — macro data is released monthly
pub const TTL_EDGAR_SECS: i64 = 604_800;  // 7 days  — 10-K filings are annual
pub const TTL_FINNHUB_SECS: i64 = 43_200; // 12 hours — insider + peer data

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheWrapper<T> {
    timestamp: i64,
    data: T,
}

fn get_cache_path(filename: &str) -> PathBuf {
    Path::new(CACHE_DIR).join(filename)
}

pub fn load_from_cache<T: DeserializeOwned>(filename: &str, ttl_secs: i64) -> Option<T> {
    let path = get_cache_path(filename);
    if !path.exists() {
        return None;
    }
    
    let file = File::open(&path).ok()?;
    let wrapper: CacheWrapper<T> = serde_json::from_reader(file).ok()?;
    
    let now = Utc::now().timestamp();
    if now - wrapper.timestamp > ttl_secs {
        log_info!("Cache expired for file: {}", filename);
        return None;
    }
    
    log_info!("Cache hit for file: {}", filename);
    Some(wrapper.data)
}

pub fn save_to_cache<T: Serialize>(filename: &str, data: &T) {
    if let Err(e) = fs::create_dir_all(CACHE_DIR) {
        log_error!("Failed to create cache directory: {}", e);
        return;
    }
    
    let path = get_cache_path(filename);
    let wrapper = CacheWrapper {
        timestamp: Utc::now().timestamp(),
        data,
    };
    
    let tmp_path = path.with_extension("tmp");
    match File::create(&tmp_path) {
        Ok(file) => {
            if let Err(e) = serde_json::to_writer_pretty(file, &wrapper) {
                log_error!("Failed to write cache file {}: {}", filename, e);
                let _ = fs::remove_file(&tmp_path);
            } else if let Err(e) = fs::rename(&tmp_path, &path) {
                log_error!("Failed to rename temp cache file to {}: {}", filename, e);
                let _ = fs::remove_file(&tmp_path);
            } else {
                log_info!("Saved to cache: {}", filename);
            }
        }
        Err(e) => {
            log_error!("Failed to create temp cache file for {}: {}", filename, e);
        }
    }
}
