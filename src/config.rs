use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::de::{self, Deserializer};
use serde::Deserialize;
use serde_json::Value;
use tiny_http::Method;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub prefix: String,
    pub base_url: String,
    pub memory_limit_bytes: u64,
    pub allowed_get_endpoints: HashSet<String>,
    pub schedules: Vec<ServiceSchedule>,
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
}

#[derive(Deserialize)]
struct RawServiceConfig {
    prefix: String,
    url: String,
    memory_limit_mb: u64,
    #[serde(default)]
    schedules: Vec<RawScheduleConfig>,
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

const BYTES_PER_MEBIBYTE: u64 = 1024 * 1024;

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

        let RawServiceConfig {
            prefix,
            url,
            memory_limit_mb,
            schedules: raw_schedules,
        } = read_service_config(&name)?;

        let memory_limit_bytes =
            memory_limit_mb
                .checked_mul(BYTES_PER_MEBIBYTE)
                .ok_or_else(|| {
                    anyhow!(
                        "memory limit for service '{}' exceeds supported range",
                        name
                    )
                })?;

        let allowed_get_endpoints = read_service_openapi(&name)?;
        let schedules = normalize_service_schedules(&name, &allowed_get_endpoints, &raw_schedules)?;

        services.push(Service {
            name,
            prefix,
            base_url: url,
            memory_limit_bytes,
            allowed_get_endpoints,
            schedules,
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

    if config.memory_limit_mb == 0 {
        bail!(
            "memory_limit_mb for service '{}' must be greater than zero",
            name
        );
    }

    Ok(())
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
    use tiny_http::Method;

    #[test]
    fn supports_only_known_get_endpoints() {
        let service = Service {
            name: "example".into(),
            prefix: "foo".into(),
            base_url: "http://localhost".into(),
            memory_limit_bytes: 64 * 1024 * 1024,
            allowed_get_endpoints: ["ping".into()].into_iter().collect(),
            schedules: Vec::new(),
        };

        assert!(service.supports(&Method::Get, "ping"));
        assert!(!service.supports(&Method::Post, "ping"));
        assert!(!service.supports(&Method::Get, "pong"));
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
