use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Service;

pub const HEALTH_POLL_INTERVAL_SECS: u64 = 5;
pub const HEALTH_REQUEST_TIMEOUT_SECS: u64 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HealthStatus {
    #[default]
    Unknown,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ServiceHealth {
    pub status: HealthStatus,
    pub last_checked: Option<Instant>,
}

pub type SharedHealthMap = Arc<Mutex<HashMap<String, ServiceHealth>>>;

pub fn start_health_monitor(services: &[Service]) -> SharedHealthMap {
    let health_map: SharedHealthMap = Arc::new(Mutex::new(HashMap::new()));

    if let Ok(mut guard) = health_map.lock() {
        for service in services {
            guard.insert(service.name.clone(), ServiceHealth::default());
        }
    }

    if services.is_empty() {
        return health_map;
    }

    let services_for_monitor = services.to_vec();
    let health_clone = Arc::clone(&health_map);

    thread::spawn(move || loop {
        for service in &services_for_monitor {
            let now = Instant::now();
            let status = perform_health_check(service);

            if let Ok(mut map) = health_clone.lock() {
                let entry = map.entry(service.name.clone()).or_default();
                entry.status = status;
                entry.last_checked = Some(now);
            }
        }

        thread::sleep(Duration::from_secs(HEALTH_POLL_INTERVAL_SECS));
    });

    health_map
}

fn perform_health_check(service: &Service) -> HealthStatus {
    let url = healthcheck_url(service);
    match ureq::get(&url)
        .timeout(Duration::from_secs(HEALTH_REQUEST_TIMEOUT_SECS))
        .call()
    {
        Ok(response) if response.status() == 200 => HealthStatus::Healthy,
        Ok(response) => {
            eprintln!(
                "Servicio '{}' respondió {} en su healthcheck",
                service.name,
                response.status()
            );
            HealthStatus::Unhealthy
        }
        Err(error) => {
            eprintln!(
                "No se pudo contactar al servicio '{}' en {}: {}",
                service.name, url, error
            );
            HealthStatus::Unhealthy
        }
    }
}

fn healthcheck_url(service: &Service) -> String {
    format!("{}/health", service.base_url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthcheck_url_trims_trailing_slashes() {
        let service = Service {
            name: "svc".into(),
            prefix: "svc".into(),
            base_url: "http://localhost:1234/".into(),
            allowed_get_endpoints: Default::default(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
        };

        assert_eq!(healthcheck_url(&service), "http://localhost:1234/health");
    }

    #[test]
    fn start_health_monitor_initializes_map() {
        let service = Service {
            name: "svc".into(),
            prefix: "svc".into(),
            base_url: "http://localhost:1234".into(),
            allowed_get_endpoints: Default::default(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
        };

        let health = start_health_monitor(&[service.clone()]);
        let map = health.lock().expect("health map");
        assert!(map.contains_key(&service.name));
        assert_eq!(map[&service.name].status, HealthStatus::Unknown);
    }
}
