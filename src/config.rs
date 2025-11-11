use std::collections::HashSet;
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
    pub allowed_get_endpoints: HashSet<String>,
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

        let RawServiceConfig { prefix, url } = read_service_config(&name)?;

        let allowed_get_endpoints = read_service_openapi(&name)?;

        services.push(Service {
            name,
            prefix,
            base_url: url,
            allowed_get_endpoints,
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
            allowed_get_endpoints: ["ping".into()].into_iter().collect(),
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
}
