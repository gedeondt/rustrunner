use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::Service;

#[derive(Clone, Copy, Debug, Default)]
pub struct ServiceMemorySnapshot {
    pub usage_bytes: Option<u64>,
    pub limit_bytes: Option<u64>,
    pub last_updated: Option<Instant>,
}

pub type SharedMemoryMap = Arc<Mutex<HashMap<String, ServiceMemorySnapshot>>>;

pub fn initialize_memory_store(services: &[Service]) -> SharedMemoryMap {
    let store: SharedMemoryMap = Arc::new(Mutex::new(HashMap::new()));

    if let Ok(mut guard) = store.lock() {
        for service in services {
            guard.insert(
                service.name.clone(),
                ServiceMemorySnapshot {
                    usage_bytes: None,
                    limit_bytes: service
                        .memory_limit_mb
                        .map(|mb| mb.saturating_mul(1024 * 1024)),
                    last_updated: None,
                },
            );
        }
    }

    store
}

pub fn record_memory_usage(store: &SharedMemoryMap, service_name: &str, usage_bytes: Option<u64>) {
    if let Ok(mut guard) = store.lock() {
        if let Some(entry) = guard.get_mut(service_name) {
            entry.usage_bytes = usage_bytes;
            entry.last_updated = Some(Instant::now());
        }
    }
}

pub fn reset_memory_entry(store: &SharedMemoryMap, service_name: &str) {
    if let Ok(mut guard) = store.lock() {
        if let Some(entry) = guard.get_mut(service_name) {
            entry.usage_bytes = None;
            entry.last_updated = None;
        }
    }
}
