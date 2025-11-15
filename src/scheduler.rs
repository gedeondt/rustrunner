use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{Service, ServiceSchedule};

pub const SCHEDULE_LOOP_TICK_SECS: u64 = 1;
pub const SCHEDULE_REQUEST_TIMEOUT_SECS: u64 = 5;

#[derive(Clone, Debug, Default)]
pub struct ScheduleState {
    pub endpoint: String,
    pub interval_secs: u64,
    pub paused: bool,
    pub last_run: Option<Instant>,
    pub last_status: Option<u16>,
    pub last_error: Option<String>,
}

pub type SharedScheduleMap = Arc<Mutex<HashMap<String, Vec<ScheduleState>>>>;

#[derive(Debug)]
pub enum ToggleError {
    ServiceNotFound,
    ScheduleNotFound,
    LockPoisoned,
}

pub fn start_webhook_schedulers(services: &[Service]) -> SharedScheduleMap {
    let schedule_map: SharedScheduleMap = Arc::new(Mutex::new(HashMap::new()));

    {
        if let Ok(mut guard) = schedule_map.lock() {
            for service in services {
                if service.schedules.is_empty() {
                    continue;
                }

                let entries = service
                    .schedules
                    .iter()
                    .map(|schedule| ScheduleState {
                        endpoint: schedule.endpoint.clone(),
                        interval_secs: schedule.interval_secs,
                        paused: false,
                        last_run: None,
                        last_status: None,
                        last_error: None,
                    })
                    .collect();

                guard.insert(service.name.clone(), entries);
            }
        }
    }

    for service in services {
        for (index, schedule) in service.schedules.iter().cloned().enumerate() {
            let shared = Arc::clone(&schedule_map);
            let service_name = service.name.clone();
            let base_url = service.base_url.clone();
            thread::spawn(move || run_schedule(shared, service_name, base_url, schedule, index));
        }
    }

    schedule_map
}

pub fn toggle_schedule(
    map: &SharedScheduleMap,
    service_name: &str,
    index: usize,
) -> Result<bool, ToggleError> {
    let mut guard = map.lock().map_err(|_| ToggleError::LockPoisoned)?;
    let entries = guard
        .get_mut(service_name)
        .ok_or(ToggleError::ServiceNotFound)?;
    let state = entries
        .get_mut(index)
        .ok_or(ToggleError::ScheduleNotFound)?;

    state.paused = !state.paused;
    Ok(state.paused)
}

#[derive(Debug)]
pub enum TriggerError {
    ServiceNotFound,
    ScheduleNotFound,
    LockPoisoned,
}

#[derive(Clone, Debug)]
pub struct TriggerOutcome {
    pub last_run: Option<Instant>,
    pub last_status: Option<u16>,
    pub last_error: Option<String>,
}

pub fn trigger_schedule_now(
    map: &SharedScheduleMap,
    service_name: &str,
    index: usize,
    base_url: &str,
    endpoint: &str,
) -> Result<TriggerOutcome, TriggerError> {
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        endpoint.trim_start_matches('/')
    );

    let result = execute_webhook(service_name, endpoint, &url);
    let (status, error_message) = match result {
        Ok(status) => (Some(status), None),
        Err(error) => (None, Some(error)),
    };

    let now = Instant::now();

    let mut guard = map.lock().map_err(|_| TriggerError::LockPoisoned)?;
    let entries = guard
        .get_mut(service_name)
        .ok_or(TriggerError::ServiceNotFound)?;
    let state = entries
        .get_mut(index)
        .ok_or(TriggerError::ScheduleNotFound)?;

    state.last_run = Some(now);
    state.last_status = status;
    state.last_error = error_message.clone();

    Ok(TriggerOutcome {
        last_run: state.last_run,
        last_status: state.last_status,
        last_error: state.last_error.clone(),
    })
}

fn run_schedule(
    schedules: SharedScheduleMap,
    service_name: String,
    base_url: String,
    schedule: ServiceSchedule,
    index: usize,
) {
    let interval = schedule.interval_secs.max(1);
    let mut remaining = interval;

    loop {
        thread::sleep(Duration::from_secs(SCHEDULE_LOOP_TICK_SECS));

        let paused = match schedules.lock() {
            Ok(guard) => match guard.get(&service_name) {
                Some(entries) => match entries.get(index) {
                    Some(state) => state.paused,
                    None => return,
                },
                None => return,
            },
            Err(_) => return,
        };

        if paused {
            remaining = interval;
            continue;
        }

        if remaining > SCHEDULE_LOOP_TICK_SECS {
            remaining -= SCHEDULE_LOOP_TICK_SECS;
            continue;
        }

        remaining = interval;

        let url = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            schedule.endpoint.trim_start_matches('/')
        );

        let result = execute_webhook(&service_name, &schedule.endpoint, &url);

        let (status, error_message) = match result {
            Ok(status) => (Some(status), None),
            Err(error) => (None, Some(error)),
        };

        if let Ok(mut guard) = schedules.lock() {
            if let Some(entries) = guard.get_mut(&service_name) {
                if let Some(state) = entries.get_mut(index) {
                    state.last_run = Some(Instant::now());
                    state.last_status = status;
                    state.last_error = error_message;
                }
            }
        } else {
            return;
        }
    }
}

fn execute_webhook(service_name: &str, endpoint: &str, url: &str) -> Result<u16, String> {
    match ureq::get(url)
        .timeout(Duration::from_secs(SCHEDULE_REQUEST_TIMEOUT_SECS))
        .call()
    {
        Ok(response) => Ok(response.status()),
        Err(ureq::Error::Status(status, response)) => {
            let _ = response.into_string();
            if status >= 400 {
                eprintln!(
                    "Scheduled webhook '/{endpoint}' for service '{service_name}' returned HTTP {status}"
                );
            }
            Ok(status)
        }
        Err(error) => {
            eprintln!(
                "Failed to execute scheduled webhook '/{endpoint}' for service '{service_name}': {error}"
            );
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_schedule_flips_state() {
        let map: SharedScheduleMap = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut guard = map.lock().unwrap();
            guard.insert(
                "svc".into(),
                vec![ScheduleState {
                    endpoint: "ping".into(),
                    interval_secs: 5,
                    paused: false,
                    last_run: None,
                    last_status: None,
                    last_error: None,
                }],
            );
        }

        assert_eq!(toggle_schedule(&map, "svc", 0).unwrap(), true);
        assert_eq!(toggle_schedule(&map, "svc", 0).unwrap(), false);
    }

    #[test]
    fn toggle_schedule_errors_when_missing() {
        let map: SharedScheduleMap = Arc::new(Mutex::new(HashMap::new()));
        assert!(matches!(
            toggle_schedule(&map, "svc", 0),
            Err(ToggleError::ServiceNotFound)
        ));
    }
}
