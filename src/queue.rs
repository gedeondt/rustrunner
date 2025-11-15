use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};

use crate::config::{Service, ServiceQueueListener};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueSubscriber {
    pub service_name: String,
    pub target_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub name: String,
    pub message_count: u64,
    pub subscriber_count: usize,
}

#[derive(Default)]
struct QueueInfo {
    subscribers: Vec<QueueSubscriber>,
    message_count: u64,
    instantiated: bool,
}

#[derive(Default)]
pub struct QueueRegistry {
    queues: HashMap<String, QueueInfo>,
}

pub type SharedQueueRegistry = Arc<Mutex<QueueRegistry>>;

impl QueueRegistry {
    fn register_listener(&mut self, queue: &str, listener: QueueSubscriber) {
        let entry = self
            .queues
            .entry(queue.to_string())
            .or_insert_with(QueueInfo::default);

        if !entry.subscribers.contains(&listener) {
            entry.subscribers.push(listener);
        }
    }

    pub fn prepare_delivery(&mut self, queue: &str) -> (Vec<QueueSubscriber>, u64) {
        let entry = self
            .queues
            .entry(queue.to_string())
            .or_insert_with(QueueInfo::default);

        entry.instantiated = true;
        entry.message_count = entry.message_count.saturating_add(1);
        (entry.subscribers.clone(), entry.message_count)
    }

    pub fn snapshot(&self) -> Vec<QueueSnapshot> {
        let mut queues: Vec<_> = self
            .queues
            .iter()
            .filter_map(|(name, info)| {
                if info.instantiated {
                    Some(QueueSnapshot {
                        name: name.clone(),
                        message_count: info.message_count,
                        subscriber_count: info.subscribers.len(),
                    })
                } else {
                    None
                }
            })
            .collect();

        queues.sort_by(|a, b| a.name.cmp(&b.name));
        queues
    }
}

pub fn initialize_queue_registry(services: &[Service]) -> SharedQueueRegistry {
    let mut registry = QueueRegistry::default();

    for service in services {
        register_service_listeners(&mut registry, service, &service.queue_listeners);
    }

    Arc::new(Mutex::new(registry))
}

fn register_service_listeners(
    registry: &mut QueueRegistry,
    service: &Service,
    listeners: &[ServiceQueueListener],
) {
    for listener in listeners {
        let target_url = format!(
            "{}/{}",
            service.base_url.trim_end_matches('/'),
            listener.path.trim_start_matches('/')
        );

        registry.register_listener(
            &listener.queue,
            QueueSubscriber {
                service_name: service.name.clone(),
                target_url,
            },
        );
    }
}

pub fn with_queue_registry<F, T>(registry: &SharedQueueRegistry, action: F) -> Result<T>
where
    F: FnOnce(&mut QueueRegistry) -> T,
{
    let mut guard = registry
        .lock()
        .map_err(|_| anyhow!("queue registry poisoned"))?;
    Ok(action(&mut guard))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceKind;
    use std::collections::HashSet;

    fn sample_service(name: &str, url: &str) -> Service {
        Service {
            name: name.into(),
            domain: "test".into(),
            kind: ServiceKind::Adapter,
            prefix: name.into(),
            base_url: url.into(),
            allowed_get_endpoints: HashSet::new(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
            memory_limit_mb: None,
        }
    }

    #[test]
    fn registers_subscribers_and_tracks_delivery() {
        let mut registry = QueueRegistry::default();
        let service = sample_service("hello", "http://localhost:1234");
        let listeners = vec![ServiceQueueListener {
            queue: "events".into(),
            path: "/hook".into(),
        }];

        register_service_listeners(&mut registry, &service, &listeners);

        let (subscribers, count) = registry.prepare_delivery("events");
        assert_eq!(count, 1);
        assert_eq!(subscribers.len(), 1);
        assert_eq!(subscribers[0].target_url, "http://localhost:1234/hook");

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].message_count, 1);
        assert_eq!(snapshot[0].subscriber_count, 1);
    }

    #[test]
    fn snapshot_excludes_non_instantiated_queues() {
        let mut registry = QueueRegistry::default();

        registry.register_listener(
            "queue",
            QueueSubscriber {
                service_name: "svc".into(),
                target_url: "http://localhost".into(),
            },
        );

        assert!(registry.snapshot().is_empty());

        registry.prepare_delivery("queue");
        assert_eq!(registry.snapshot().len(), 1);
    }
}
