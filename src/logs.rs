use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::config::Service;

pub const MAX_STORED_LOG_LINES: usize = 200;

pub type SharedLogMap = Arc<Mutex<HashMap<String, VecDeque<String>>>>;

pub fn initialize_log_store(services: &[Service]) -> SharedLogMap {
    let store: SharedLogMap = Arc::new(Mutex::new(HashMap::new()));

    if let Ok(mut guard) = store.lock() {
        for service in services {
            guard.insert(service.name.clone(), VecDeque::new());
        }
    }

    store
}

pub fn spawn_log_forwarder<R>(
    service_name: String,
    reader: R,
    stream_label: &'static str,
    logs: SharedLogMap,
) where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            match line {
                Ok(line) => {
                    let (level, message) = parse_service_log_line(&line)
                        .map(|(level, message)| (level.to_string(), message.to_string()))
                        .unwrap_or_else(|| (stream_label.to_uppercase(), line));
                    let formatted = format!("[svc:{}][{}] {}", service_name, level, message);
                    println!("{}", formatted);
                    if let Ok(mut guard) = logs.lock() {
                        let entry = guard.entry(service_name.clone()).or_default();
                        entry.push_back(formatted);
                        while entry.len() > MAX_STORED_LOG_LINES {
                            entry.pop_front();
                        }
                    }
                }
                Err(error) => {
                    eprintln!(
                        "failed to read {stream_label} from service '{}': {}",
                        service_name, error
                    );
                    break;
                }
            }
        }
    });
}

pub fn parse_service_log_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let (level, remainder) = rest.split_at(end);
    let message = remainder.get(1..).unwrap_or_default().trim_start();
    Some((level, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expected_log_format() {
        let (level, message) =
            parse_service_log_line("[INFO] Something happened").expect("parse log line");
        assert_eq!(level, "INFO");
        assert_eq!(message, "Something happened");
    }

    #[test]
    fn returns_none_for_unexpected_format() {
        assert!(parse_service_log_line("INFO Something").is_none());
    }
}
