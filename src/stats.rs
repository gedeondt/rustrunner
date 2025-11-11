use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

const MAX_MINUTES: u64 = 60;

pub type SharedStats = Arc<Mutex<StatsStore>>;

#[derive(Debug, Default)]
pub struct StatsStore {
    data: HashMap<String, HashMap<String, BTreeMap<u64, HashMap<u16, u32>>>>,
}

#[derive(Debug, Default, Serialize)]
pub struct StatsSnapshot {
    pub generated_at: u64,
    pub window_minutes: u64,
    pub global: Vec<MinuteAggregate>,
    pub services: Vec<ServiceSnapshot>,
}

#[derive(Debug, Default, Serialize)]
pub struct ServiceSnapshot {
    pub service: String,
    pub endpoints: Vec<EndpointSnapshot>,
}

#[derive(Debug, Default, Serialize)]
pub struct EndpointSnapshot {
    pub endpoint: String,
    pub minutes: Vec<MinuteAggregate>,
}

#[derive(Debug, Default, Serialize)]
pub struct MinuteAggregate {
    pub minute: u64,
    pub counts: BTreeMap<u16, u32>,
}

impl StatsStore {
    pub fn record(&mut self, service: &str, endpoint: &str, status: u16, timestamp: SystemTime) {
        let Ok(elapsed) = timestamp.duration_since(UNIX_EPOCH) else {
            return;
        };

        let minute = elapsed.as_secs() / 60;

        let service_entry = self
            .data
            .entry(service.to_string())
            .or_insert_with(HashMap::new);

        let endpoint_entry = service_entry
            .entry(endpoint.to_string())
            .or_insert_with(BTreeMap::new);

        let bucket = endpoint_entry.entry(minute).or_insert_with(HashMap::new);
        *bucket.entry(status).or_insert(0) += 1;

        let cutoff = minute.saturating_sub(MAX_MINUTES - 1);
        endpoint_entry.retain(|minute_key, _| *minute_key >= cutoff);
    }

    pub fn snapshot(&self, now: SystemTime) -> StatsSnapshot {
        let generated_at = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();

        let mut global_map: BTreeMap<u64, BTreeMap<u16, u32>> = BTreeMap::new();
        let mut services = Vec::new();

        for (service_name, endpoints) in &self.data {
            let mut endpoint_snapshots = Vec::new();

            for (endpoint_name, minutes) in endpoints {
                let mut minute_snapshots = Vec::new();

                for (minute, counts) in minutes {
                    let mut sorted_counts = BTreeMap::new();
                    for (status, count) in counts {
                        sorted_counts.insert(*status, *count);

                        let global_counts = global_map.entry(*minute).or_default();
                        *global_counts.entry(*status).or_insert(0) += *count;
                    }

                    minute_snapshots.push(MinuteAggregate {
                        minute: *minute,
                        counts: sorted_counts,
                    });
                }

                minute_snapshots.sort_by_key(|snapshot| snapshot.minute);

                if !minute_snapshots.is_empty() {
                    endpoint_snapshots.push(EndpointSnapshot {
                        endpoint: endpoint_name.clone(),
                        minutes: minute_snapshots,
                    });
                }
            }

            endpoint_snapshots.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));

            if !endpoint_snapshots.is_empty() {
                services.push(ServiceSnapshot {
                    service: service_name.clone(),
                    endpoints: endpoint_snapshots,
                });
            }
        }

        services.sort_by(|a, b| a.service.cmp(&b.service));

        let global = global_map
            .into_iter()
            .map(|(minute, counts)| MinuteAggregate { minute, counts })
            .collect();

        StatsSnapshot {
            generated_at,
            window_minutes: MAX_MINUTES,
            global,
            services,
        }
    }
}

pub fn initialize_stats_store() -> SharedStats {
    Arc::new(Mutex::new(StatsStore::default()))
}

pub fn record_http_status(stats: &SharedStats, service: &str, endpoint: &str, status: u16) {
    if let Ok(mut guard) = stats.lock() {
        guard.record(service, endpoint, status, SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minute_time(minutes_after_epoch: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(minutes_after_epoch * 60)
    }

    #[test]
    fn records_statuses_by_minute_and_service() {
        let mut store = StatsStore::default();
        store.record("svc-a", "ping", 200, minute_time(0));
        store.record("svc-a", "ping", 200, minute_time(0));
        store.record("svc-a", "ping", 404, minute_time(1));
        store.record("svc-b", "health", 500, minute_time(1));

        let snapshot = store.snapshot(minute_time(1));

        assert_eq!(snapshot.global.len(), 2);
        assert_eq!(snapshot.services.len(), 2);

        let first_minute = &snapshot.global[0];
        assert_eq!(first_minute.minute, 0);
        assert_eq!(first_minute.counts.get(&200), Some(&2));

        let svc_a = snapshot
            .services
            .iter()
            .find(|service| service.service == "svc-a")
            .expect("service a");
        let ping = svc_a
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint == "ping")
            .expect("ping endpoint");
        assert_eq!(ping.minutes.len(), 2);
        assert_eq!(ping.minutes[0].counts.get(&200), Some(&2));
    }

    #[test]
    fn prunes_entries_older_than_an_hour() {
        let mut store = StatsStore::default();
        // Record an entry exactly 61 minutes apart.
        store.record("svc", "ping", 200, minute_time(0));
        store.record("svc", "ping", 200, minute_time(61));

        let snapshot = store.snapshot(minute_time(61));
        assert_eq!(snapshot.global.len(), 1);
        assert_eq!(snapshot.global[0].minute, 61);
    }
}
