use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};

const ENTRY_PORT: u16 = 14000;
const SERVICE_NAMES: &[&str] = &["hello_world", "bye_world"];

struct Service {
    name: &'static str,
    prefix: String,
    base_url: String,
}

#[derive(Deserialize)]
struct RawServiceConfig {
    prefix: String,
    url: String,
}

fn main() -> Result<()> {
    let services = load_services()?;

    let server = Server::http(("0.0.0.0", ENTRY_PORT)).map_err(|error| {
        anyhow!(
            "failed to bind entrypoint to port {}: {}",
            ENTRY_PORT,
            error
        )
    })?;

    println!("Runner listening on http://{}:{}", "0.0.0.0", ENTRY_PORT);

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(&services, request) {
            eprintln!("Failed to handle request: {:#}", error);
        }
    }

    Ok(())
}

fn load_services() -> Result<Vec<Service>> {
    let mut services = Vec::new();

    for &name in SERVICE_NAMES {
        let RawServiceConfig { prefix, url } = read_service_config(name)?;

        services.push(Service {
            name,
            prefix,
            base_url: url,
        });
    }

    Ok(services)
}

fn handle_request(services: &[Service], request: Request) -> Result<()> {
    if request.method() != &Method::Get {
        let response = Response::from_string("method not allowed").with_status_code(405);
        request.respond(response)?;
        return Ok(());
    }

    let full_path = request.url();
    let (path, query) = match full_path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (full_path, None),
    };

    let mut segments = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty());

    let Some(prefix) = segments.next() else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    let Some(service) = services.iter().find(|service| service.prefix == prefix) else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    let Some(endpoint) = segments.next() else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    if segments.next().is_some() {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let mut target_url = format!(
        "{}/{}",
        service.base_url.trim_end_matches('/'),
        endpoint
    );

    if let Some(query) = query {
        target_url.push('?');
        target_url.push_str(query);
    }

    match ureq::request(request.method().as_str(), &target_url).call() {
        Ok(response) => {
            let response = build_response(response)?;
            request.respond(response)?;
        }
        Err(ureq::Error::Status(_, response)) => {
            let response = build_response(response)?;
            request.respond(response)?;
        }
        Err(error) => {
            eprintln!(
                "Error contacting service '{}': {}",
                service.name, error
            );
            let response = Response::from_string("upstream error").with_status_code(502);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn build_response(upstream: ureq::Response) -> Result<Response<Cursor<Vec<u8>>>> {
    let status = upstream.status();
    let content_type = upstream.header("Content-Type").map(|value| value.to_owned());
    let body = upstream.into_string().context("failed to read upstream response body")?;

    let mut response = Response::from_string(body).with_status_code(status);

    if let Some(content_type) = content_type {
        if let Ok(header) = Header::from_bytes(b"Content-Type", content_type.as_bytes()) {
            response = response.with_header(header);
        }
    }

    Ok(response)
}

fn config_path(name: &str) -> PathBuf {
    PathBuf::from("services")
        .join(name)
        .join("config")
        .join("service.json")
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

    if config.prefix.trim().is_empty() {
        bail!("prefix for service '{}' cannot be empty", name);
    }

    if config.url.trim().is_empty() {
        bail!("url for service '{}' cannot be empty", name);
    }

    Ok(config)
}
