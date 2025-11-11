use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
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
    pub queue_listeners: Vec<ServiceQueueListener>,
}

impl Service {
    pub fn supports(&self, method: &Method, endpoint: &str) -> bool {
        matches!(method, &Method::Get) && self.allowed_get_endpoints.contains(endpoint)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceQueueListener {
    pub queue: String,
    pub path: String,
}

#[derive(Deserialize)]
struct RawServiceConfig {
    prefix: String,
    url: String,
    memory_limit_mb: u64,
    #[serde(default)]
    listeners: Vec<HashMap<String, String>>,
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
            listeners,
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
        let queue_listeners = parse_queue_listeners(&name, &listeners)
            .with_context(|| format!("failed to parse queue listeners for service '{}'", name))?;

        services.push(Service {
            name,
            prefix,
            base_url: url,
            memory_limit_bytes,
            allowed_get_endpoints,
            queue_listeners,
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
    use std::collections::HashMap;
    use tiny_http::Method;

    #[test]
    fn supports_only_known_get_endpoints() {
        let service = Service {
            name: "example".into(),
            prefix: "foo".into(),
            base_url: "http://localhost".into(),
            memory_limit_bytes: 64 * 1024 * 1024,
            allowed_get_endpoints: ["ping".into()].into_iter().collect(),
            queue_listeners: Vec::new(),
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
}
