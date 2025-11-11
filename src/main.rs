use std::fs;
use std::io::{Cursor, ErrorKind};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

const ENTRY_PORT: u16 = 14000;
const SERVICE_NAMES: &[&str] = &["hello_world", "bye_world"];
const SERVICE_STARTUP_ATTEMPTS: usize = 50;
const SERVICE_STARTUP_BACKOFF_MS: u64 = 100;

struct Service {
    name: &'static str,
    prefix: String,
    base_url: String,
}

struct ServiceGuard {
    name: &'static str,
    child: Child,
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = self.child.kill() {
                    if error.kind() != ErrorKind::InvalidInput {
                        eprintln!("failed to stop service '{}': {}", self.name, error);
                    }
                }

                if let Err(error) = self.child.wait() {
                    if error.kind() != ErrorKind::InvalidInput {
                        eprintln!("failed to wait for service '{}': {}", self.name, error);
                    }
                }
            }
            Err(error) => {
                eprintln!(
                    "failed to determine status of service '{}': {}",
                    self.name, error
                );
            }
        }
    }
}

#[derive(Deserialize)]
struct RawServiceConfig {
    prefix: String,
    url: String,
}

fn main() -> Result<()> {
    let services = load_services()?;
    let _service_guards = start_service_processes(&services)?;

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

fn start_service_processes(services: &[Service]) -> Result<Vec<ServiceGuard>> {
    let mut guards = Vec::new();

    for service in services {
        if probe_service(service).is_ok() {
            println!(
                "Service '{}' already running at {}",
                service.name, service.base_url
            );
            continue;
        }

        let manifest_path = service_manifest_path(service.name);
        println!(
            "Starting service '{}' using manifest {}",
            service.name,
            manifest_path.display()
        );

        let mut child = Command::new("cargo")
            .arg("run")
            .arg("--manifest-path")
            .arg(&manifest_path)
            .spawn()
            .with_context(|| format!("failed to start service '{}' via cargo", service.name))?;

        match wait_for_service(service) {
            Ok(()) => {
                guards.push(ServiceGuard {
                    name: service.name,
                    child,
                });
            }
            Err(error) => {
                if let Err(kill_error) = child.kill() {
                    if kill_error.kind() != ErrorKind::InvalidInput {
                        eprintln!(
                            "failed to stop service '{}' after startup error: {}",
                            service.name, kill_error
                        );
                    }
                }
                let _ = child.wait();
                return Err(error);
            }
        }
    }

    Ok(guards)
}

fn wait_for_service(service: &Service) -> Result<()> {
    let mut last_error = None;

    for _ in 0..SERVICE_STARTUP_ATTEMPTS {
        match probe_service(service) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(SERVICE_STARTUP_BACKOFF_MS));
            }
        }
    }

    if let Some(error) = last_error {
        return Err(error).context(format!(
            "service '{}' did not become ready in time",
            service.name
        ));
    }

    bail!("service '{}' did not become ready in time", service.name)
}

fn probe_service(service: &Service) -> Result<()> {
    let url = Url::parse(&service.base_url).with_context(|| {
        format!(
            "invalid base URL '{}' for service '{}'",
            service.base_url, service.name
        )
    })?;

    let host = url.host_str().ok_or_else(|| {
        anyhow!(
            "service '{}' base URL missing host: {}",
            service.name,
            service.base_url
        )
    })?;

    let port = url.port_or_known_default().ok_or_else(|| {
        anyhow!(
            "service '{}' base URL missing port: {}",
            service.name,
            service.base_url
        )
    })?;

    let mut last_error = None;

    for address in (host, port).to_socket_addrs().with_context(|| {
        format!(
            "failed to resolve address for service '{}' at {}",
            service.name, service.base_url
        )
    })? {
        match TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    if let Some(error) = last_error {
        let context = format!(
            "failed to connect to service '{}' at {}",
            service.name, service.base_url
        );
        return Err(anyhow!(error)).context(context);
    }

    bail!(
        "service '{}' resolved to no addresses at {}",
        service.name,
        service.base_url
    )
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

    let mut target_url = format!("{}/{}", service.base_url.trim_end_matches('/'), endpoint);

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
            eprintln!("Error contacting service '{}': {}", service.name, error);
            let response = Response::from_string("upstream error").with_status_code(502);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn build_response(upstream: ureq::Response) -> Result<Response<Cursor<Vec<u8>>>> {
    let status = upstream.status();
    let content_type = upstream
        .header("Content-Type")
        .map(|value| value.to_owned());
    let body = upstream
        .into_string()
        .context("failed to read upstream response body")?;

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

fn service_manifest_path(name: &str) -> PathBuf {
    PathBuf::from("services").join(name).join("Cargo.toml")
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
