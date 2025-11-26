use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::de::{self, Deserializer};
use serde::Deserialize;
use serde_json::Value;
use tiny_http::Method;
use url::Url;

const MAX_MEMORY_LIMIT_MB: u64 = (u32::MAX as u64) / 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub domain: String,
    pub kind: ServiceKind,
    pub prefix: String,
    pub base_url: String,
    pub runner_urls: Vec<String>,
    pub allowed_get_endpoints: HashSet<String>,
    pub queue_listeners: Vec<ServiceQueueListener>,
    pub schedules: Vec<ServiceSchedule>,
    pub memory_limit_mb: Option<u64>,
    pub runner_instances: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSchedule {
    pub endpoint: String,
    pub interval_secs: u64,
}

impl Service {
    pub fn supports(&self, method: &Method, endpoint: &str) -> bool {
        matches!(method, &Method::Get) && self.allowed_get_endpoints.contains(endpoint)
    }

    pub fn runner_count(&self) -> usize {
        self.runner_urls.len().max(1)
    }

    pub fn runner_endpoints(&self) -> &[String] {
        if self.runner_urls.is_empty() {
            std::slice::from_ref(&self.base_url)
        } else {
            &self.runner_urls
        }
    }

    pub fn memory_page_limit(&self) -> Option<u32> {
        self.memory_limit_mb
            .and_then(|limit| limit.checked_mul(16))
            .and_then(|pages| pages.try_into().ok())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceQueueListener {
    pub queue: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Bff,
    Business,
    Adapter,
}

impl ServiceKind {
    pub fn label(&self) -> &'static str {
        match self {
            ServiceKind::Bff => "BFF",
            ServiceKind::Business => "Business",
            ServiceKind::Adapter => "Adapter",
        }
    }
}

#[derive(Deserialize)]
struct RawServiceConfig {
    prefix: String,
    url: String,
    domain: String,
    #[serde(rename = "type")]
    kind: ServiceKind,
    #[serde(default = "default_runner_instances")]
    runners: usize,
    #[serde(default)]
    memory_limit_mb: Option<u64>,
    #[serde(default)]
    listeners: Vec<HashMap<String, String>>,
    #[serde(default)]
    schedules: Vec<RawScheduleConfig>,
}

fn default_runner_instances() -> usize {
    1
}

#[derive(Debug, Clone)]
struct RawScheduleConfig {
    endpoint: String,
    interval_secs: u64,
}

impl<'de> Deserialize<'de> for RawScheduleConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value {
            Value::Array(items) => parse_schedule_from_array(items).map_err(de::Error::custom),
            Value::Object(map) => parse_schedule_from_object(map).map_err(de::Error::custom),
            other => Err(de::Error::custom(format!(
                "schedule entry must be an array or object, found {other:?}"
            ))),
        }
    }
}

pub fn load_services() -> Result<Vec<Service>> {
    let mut services = Vec::new();
    let services_dir = Path::new("services");

    if !services_dir.exists() {
        println!(
            "Services directory '{}' not found. No services will be loaded.",
            services_dir.display()
        );
        return Ok(services);
    }

    for entry in fs::read_dir(services_dir).with_context(|| {
        format!(
            "failed to read services directory at {}",
            services_dir.display()
        )
    })? {
        let entry = entry?;

        if !entry.file_type()?.is_dir() {
            continue;
        }

        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            eprintln!(
                "Skipping service with non-unicode name in {}",
                entry.path().display()
            );
            continue;
        };

        let name = name.to_owned();
        let config_path = config_path(&name);

        if !config_path.exists() {
            println!(
                "Skipping service '{}' because configuration was not found at {}",
                name,
                config_path.display()
            );
            continue;
        }

        let manifest_path = service_manifest_path(&name);

        if !manifest_path.exists() {
            println!(
                "Skipping service '{}' because manifest was not found at {}",
                name,
                manifest_path.display()
            );
            continue;
        }

        let RawServiceConfig {
            prefix,
            url,
            domain,
            kind,
            runners,
            memory_limit_mb,
            listeners,
            schedules: raw_schedules,
        } = read_service_config(&name)?;

        let allowed_get_endpoints = read_service_openapi(&name)?;
        let queue_listeners = parse_queue_listeners(&name, &listeners)
            .with_context(|| format!("failed to parse queue listeners for service '{}'", name))?;
        let schedules = normalize_service_schedules(&name, &allowed_get_endpoints, &raw_schedules)?;

        let runner_urls = build_runner_urls(&name, &url, runners)?;
        let base_url = runner_urls
            .first()
            .cloned()
            .unwrap_or_else(|| url.trim_end_matches('/').to_string());
        let runner_instances = runner_urls.len();

        services.push(Service {
            name,
            domain,
            kind,
            prefix,
            base_url,
            runner_urls,
            allowed_get_endpoints,
            queue_listeners,
            schedules,
            memory_limit_mb,
            runner_instances,
        });
    }

    services.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(services)
}

pub fn config_path(name: &str) -> PathBuf {
    PathBuf::from("services")
        .join(name)
        .join("config")
        .join("service.json")
}

pub fn service_manifest_path(name: &str) -> PathBuf {
    PathBuf::from("services").join(name).join("Cargo.toml")
}

pub fn openapi_path(name: &str) -> PathBuf {
    PathBuf::from("services").join(name).join("openapi.json")
}

fn read_service_config(name: &str) -> Result<RawServiceConfig> {
    let path = config_path(name);
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read configuration for service '{}' at {}",
            name,
            path.display()
        )
    })?;

    let config: RawServiceConfig = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse configuration for service '{}' at {}",
            name,
            path.display()
        )
    })?;

    validate_service_config(name, &config)?;

    Ok(config)
}

fn validate_service_config(name: &str, config: &RawServiceConfig) -> Result<()> {
    if config.prefix.trim().is_empty() {
        bail!("prefix for service '{}' cannot be empty", name);
    }

    if config.url.trim().is_empty() {
        bail!("url for service '{}' cannot be empty", name);
    }

    if config.domain.trim().is_empty() {
        bail!("domain for service '{}' cannot be empty", name);
    }

    if config.runners == 0 {
        bail!("runners for service '{name}' must be at least 1");
    }

    if let Some(limit) = config.memory_limit_mb {
        if limit == 0 {
            bail!("memory_limit_mb for service '{name}' must be greater than zero");
        }

        if limit > MAX_MEMORY_LIMIT_MB {
            bail!(
                "memory_limit_mb for service '{name}' exceeds supported maximum of {MAX_MEMORY_LIMIT_MB} MB"
            );
        }
    }

    Ok(())
}

fn build_runner_urls(name: &str, url: &str, runners: usize) -> Result<Vec<String>> {
    let normalized = url.trim();
    if normalized.is_empty() {
        bail!("url for service '{name}' cannot be empty");
    }

    if runners == 1 {
        return Ok(vec![normalized.trim_end_matches('/').to_string()]);
    }

    let parsed = Url::parse(normalized).with_context(|| {
        format!(
            "failed to parse URL '{}' for service '{}'",
            normalized, name
        )
    })?;
    let Some(start_port) = parsed.port_or_known_default() else {
        bail!(
            "service '{name}' must include a port in the URL to run multiple instances (found '{}')",
            normalized
        );
    };

    let mut urls = Vec::with_capacity(runners);
    for offset in 0..runners {
        let port = start_port as u32 + offset as u32;
        if port > u16::MAX as u32 {
            bail!(
                "service '{name}' cannot allocate runner #{} because it would exceed TCP port range",
                offset + 1
            );
        }

        let mut clone = parsed.clone();
        clone.set_port(Some(port as u16)).map_err(|_| {
            anyhow!(
                "failed to set port for runner #{} of service '{name}'",
                offset + 1
            )
        })?;
        let serialized: String = clone.into();
        urls.push(serialized.trim_end_matches('/').to_string());
    }

    Ok(urls)
}

fn parse_queue_listeners(
    service_name: &str,
    raw_listeners: &[HashMap<String, String>],
) -> Result<Vec<ServiceQueueListener>> {
    let mut listeners = Vec::new();

    for entry in raw_listeners {
        if entry.len() != 1 {
            bail!(
                "listener entries for service '{}' must contain exactly one queue mapping",
                service_name
            );
        }

        for (queue, path) in entry {
            let queue = queue.trim();
            if queue.is_empty() {
                bail!(
                    "listener for service '{}' declares an empty queue name",
                    service_name
                );
            }

            let path = path.trim();
            if path.is_empty() {
                bail!(
                    "listener for service '{}' declares an empty callback path",
                    service_name
                );
            }

            if !path.starts_with('/') {
                bail!(
                    "listener for service '{}' must declare callback paths starting with '/'",
                    service_name
                );
            }

            listeners.push(ServiceQueueListener {
                queue: queue.to_string(),
                path: path.to_string(),
            });
        }
    }

    Ok(listeners)
}

fn read_service_openapi(name: &str) -> Result<HashSet<String>> {
    let path = openapi_path(name);
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read OpenAPI specification for service '{}' at {}",
            name,
            path.display()
        )
    })?;

    let document: Value = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse OpenAPI specification for service '{}' at {}",
            name,
            path.display()
        )
    })?;

    collect_get_endpoints(&document).with_context(|| {
        format!(
            "OpenAPI specification for service '{}' missing 'paths' object",
            name
        )
    })
}

fn parse_schedule_from_array(items: Vec<Value>) -> Result<RawScheduleConfig, String> {
    if items.len() != 2 {
        return Err("schedule array must contain exactly two items".to_string());
    }

    let endpoint = items
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| "schedule array first item must be a string endpoint".to_string())?
        .to_owned();

    let interval_value = items
        .get(1)
        .ok_or_else(|| "schedule array second item must be an interval".to_string())?;

    let interval_secs = parse_interval_value(interval_value)?;

    Ok(RawScheduleConfig {
        endpoint,
        interval_secs,
    })
}

fn parse_schedule_from_object(
    mut map: serde_json::Map<String, Value>,
) -> Result<RawScheduleConfig, String> {
    if map.len() == 1 {
        if let Some((key, value)) = map.iter().next() {
            let special_key = matches!(
                key.as_str(),
                "endpoint" | "path" | "interval" | "interval_secs" | "seconds" | "every_secs"
            );

            if !special_key {
                let interval_secs = parse_interval_value(value)?;
                return Ok(RawScheduleConfig {
                    endpoint: key.clone(),
                    interval_secs,
                });
            }
        }
    }

    let endpoint_value = map
        .remove("endpoint")
        .or_else(|| map.remove("path"))
        .ok_or_else(|| "schedule object missing 'endpoint' field".to_string())?;

    let endpoint = endpoint_value
        .as_str()
        .ok_or_else(|| "schedule 'endpoint' must be a string".to_string())?
        .to_owned();

    let interval_value = map
        .remove("interval_secs")
        .or_else(|| map.remove("seconds"))
        .or_else(|| map.remove("interval"))
        .or_else(|| map.remove("every_secs"))
        .ok_or_else(|| "schedule object missing interval field".to_string())?;

    let interval_secs = parse_interval_value(&interval_value)?;

    Ok(RawScheduleConfig {
        endpoint,
        interval_secs,
    })
}

fn parse_interval_value(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| "schedule interval must be a positive integer".to_string())
}

fn normalize_service_schedules(
    service_name: &str,
    allowed_endpoints: &HashSet<String>,
    raw_schedules: &[RawScheduleConfig],
) -> Result<Vec<ServiceSchedule>> {
    let mut schedules = Vec::new();

    for (idx, raw) in raw_schedules.iter().enumerate() {
        let endpoint = raw.endpoint.trim().trim_matches('/');

        if endpoint.is_empty() {
            bail!(
                "schedule entry #{idx} for service '{service_name}' must declare a non-empty endpoint"
            );
        }

        if raw.interval_secs == 0 {
            bail!(
                "schedule entry '/{endpoint}' for service '{service_name}' must declare an interval greater than zero"
            );
        }

        if !allowed_endpoints.contains(endpoint) {
            bail!(
                "schedule endpoint '/{endpoint}' for service '{service_name}' is not declared in its OpenAPI document"
            );
        }

        schedules.push(ServiceSchedule {
            endpoint: endpoint.to_string(),
            interval_secs: raw.interval_secs,
        });
    }

    Ok(schedules)
}

fn collect_get_endpoints(document: &Value) -> Result<HashSet<String>> {
    let paths = document
        .get("paths")
        .and_then(|value| value.as_object())
        .ok_or_else(|| anyhow!("document missing 'paths'"))?;

    let mut allowed = HashSet::new();

    for (path_key, methods_value) in paths {
        let Some(methods) = methods_value.as_object() else {
            continue;
        };

        let allows_get = methods
            .keys()
            .any(|method| method.eq_ignore_ascii_case("get"));

        if !allows_get {
            continue;
        }

        let endpoint = path_key.trim_matches('/');

        if endpoint.is_empty() {
            continue;
        }

        allowed.insert(endpoint.to_string());
    }

    if allowed.is_empty() {
        bail!("document does not declare any GET endpoints");
    }

    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use tiny_http::Method;

    #[test]
    fn supports_only_known_get_endpoints() {
        let service = Service {
            name: "example".into(),
            domain: "demo".into(),
            kind: ServiceKind::Business,
            prefix: "foo".into(),
            base_url: "http://localhost".into(),
            runner_urls: vec!["http://localhost".into()],
            allowed_get_endpoints: ["ping".into()].into_iter().collect(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
            memory_limit_mb: None,
            runner_instances: 1,
        };

        assert!(service.supports(&Method::Get, "ping"));
        assert!(!service.supports(&Method::Post, "ping"));
        assert!(!service.supports(&Method::Get, "pong"));
    }

    #[test]
    fn converts_memory_limit_to_pages() {
        let mut service = Service {
            name: "example".into(),
            domain: "demo".into(),
            kind: ServiceKind::Business,
            prefix: "foo".into(),
            base_url: "http://localhost".into(),
            runner_urls: vec!["http://localhost".into()],
            allowed_get_endpoints: HashSet::new(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
            memory_limit_mb: Some(100),
            runner_instances: 1,
        };

        assert_eq!(service.memory_page_limit(), Some(1600));

        service.memory_limit_mb = None;
        assert_eq!(service.memory_page_limit(), None);
    }

    #[test]
    fn collects_get_endpoints_ignoring_non_get_methods() {
        let document = json!({
            "paths": {
                "/ping": {
                    "get": {},
                    "post": {}
                },
                "/health": {
                    "post": {}
                },
                "/": {
                    "get": {}
                }
            }
        });

        let endpoints = collect_get_endpoints(&document).expect("collect endpoints");
        assert_eq!(endpoints, ["ping".into()].into_iter().collect());
    }

    #[test]
    fn fails_when_no_get_endpoints_declared() {
        let document = json!({
            "paths": {
                "/health": {
                    "post": {}
                }
            }
        });

        let error = collect_get_endpoints(&document).unwrap_err();
        assert!(error.to_string().contains("does not declare any GET"));
    }

    #[test]
    fn parses_queue_listeners_enforcing_invariants() {
        let listeners = vec![HashMap::from([(
            String::from("queue"),
            String::from("/hook"),
        )])];
        let parsed = parse_queue_listeners("demo", &listeners).expect("parse listeners");

        assert_eq!(
            parsed,
            vec![ServiceQueueListener {
                queue: "queue".into(),
                path: "/hook".into(),
            }]
        );

        let invalid = vec![HashMap::from([(String::from(""), String::from("/hook"))])];
        assert!(parse_queue_listeners("demo", &invalid).is_err());

        let invalid_path = vec![HashMap::from([(
            String::from("queue"),
            String::from("hook"),
        )])];
        assert!(parse_queue_listeners("demo", &invalid_path).is_err());
    }

    #[test]
    fn parse_schedule_entries_from_multiple_formats() {
        let raw: Vec<RawScheduleConfig> = serde_json::from_value(json!([
            ["/ping", 30],
            {"endpoint": "hello", "seconds": 45},
            {"/health": 5}
        ]))
        .expect("parse schedules");

        let allowed = ["ping", "hello", "health"]
            .into_iter()
            .map(String::from)
            .collect();

        let schedules =
            normalize_service_schedules("svc", &allowed, &raw).expect("normalize schedules");

        assert_eq!(schedules.len(), 3);
        assert_eq!(schedules[0].endpoint, "ping");
        assert_eq!(schedules[0].interval_secs, 30);
        assert_eq!(schedules[1].endpoint, "hello");
        assert_eq!(schedules[1].interval_secs, 45);
        assert_eq!(schedules[2].endpoint, "health");
        assert_eq!(schedules[2].interval_secs, 5);
    }

    #[test]
    fn normalize_schedules_rejects_unknown_endpoint() {
        let raw: Vec<RawScheduleConfig> =
            serde_json::from_value(json!([["/unknown", 10]])).expect("parse schedules");

        let allowed = ["ping"].into_iter().map(String::from).collect();

        let error = normalize_service_schedules("svc", &allowed, &raw).unwrap_err();
        assert!(error
            .to_string()
            .contains("is not declared in its OpenAPI document"));
    }
}
