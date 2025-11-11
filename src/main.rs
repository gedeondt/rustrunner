use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Cursor, ErrorKind};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

const ENTRY_PORT: u16 = 14000;
const SERVICE_STARTUP_ATTEMPTS: usize = 50;
const SERVICE_STARTUP_BACKOFF_MS: u64 = 100;
const HEALTH_POLL_INTERVAL_SECS: u64 = 5;
const HEALTH_REQUEST_TIMEOUT_SECS: u64 = 2;

#[derive(Clone)]
struct Service {
    name: String,
    prefix: String,
    base_url: String,
    allowed_get_endpoints: HashSet<String>,
}

impl Service {
    fn supports(&self, method: &Method, endpoint: &str) -> bool {
        matches!(method, &Method::Get) && self.allowed_get_endpoints.contains(endpoint)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum HealthStatus {
    #[default]
    Unknown,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Copy, Default)]
struct ServiceHealth {
    status: HealthStatus,
    last_checked: Option<Instant>,
}

type SharedHealthMap = Arc<Mutex<HashMap<String, ServiceHealth>>>;

struct ServiceGuard {
    name: String,
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
    let health = start_health_monitor(&services);

    let server = Server::http(("0.0.0.0", ENTRY_PORT)).map_err(|error| {
        anyhow!(
            "failed to bind entrypoint to port {}: {}",
            ENTRY_PORT,
            error
        )
    })?;

    println!("Runner listening on http://{}:{}", "0.0.0.0", ENTRY_PORT);

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(&services, &health, request) {
            eprintln!("Failed to handle request: {:#}", error);
        }
    }

    Ok(())
}

fn load_services() -> Result<Vec<Service>> {
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

        let manifest_path = service_manifest_path(&service.name);
        println!(
            "Starting service '{}' using manifest {}",
            service.name,
            manifest_path.display()
        );

        let mut child = Command::new("cargo")
            .arg("run")
            .arg("--manifest-path")
            .arg(&manifest_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start service '{}' via cargo", service.name))?;

        if let Some(stdout) = child.stdout.take() {
            spawn_log_forwarder(service.name.clone(), stdout, "stdout");
        }

        if let Some(stderr) = child.stderr.take() {
            spawn_log_forwarder(service.name.clone(), stderr, "stderr");
        }

        match wait_for_service(service) {
            Ok(()) => {
                guards.push(ServiceGuard {
                    name: service.name.clone(),
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

fn spawn_log_forwarder<R>(service_name: String, reader: R, stream_label: &'static str)
where
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
                    println!("[svc:{}][{}] {}", service_name, level, message);
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

fn parse_service_log_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let (level, remainder) = rest.split_at(end);
    let message = remainder.get(1..).unwrap_or_default().trim_start();
    Some((level, message))
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

fn handle_request(services: &[Service], health: &SharedHealthMap, request: Request) -> Result<()> {
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

    let trimmed_path = path.trim_start_matches('/');

    if trimmed_path == "health" {
        let response = Response::from_string("ok").with_status_code(200);
        request.respond(response)?;
        return Ok(());
    }

    if trimmed_path.is_empty() {
        let response = render_homepage(services, health);
        request.respond(response)?;
        return Ok(());
    }

    let mut segments = trimmed_path
        .split('/')
        .filter(|segment| !segment.is_empty());

    let Some(prefix) = segments.next() else {
        let response = render_homepage(services, health);
        request.respond(response)?;
        return Ok(());
    };

    let Some(service) = services.iter().find(|service| service.prefix == prefix) else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    let endpoint_segments: Vec<_> = segments.collect();

    if endpoint_segments.is_empty() {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let endpoint_path = endpoint_segments.join("/");

    if !service.supports(request.method(), &endpoint_path) {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let mut target_url = format!(
        "{}/{}",
        service.base_url.trim_end_matches('/'),
        endpoint_path
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

fn render_homepage(services: &[Service], health: &SharedHealthMap) -> Response<Cursor<Vec<u8>>> {
    let service_section = if services.is_empty() {
        "<p>No hay servicios cargados actualmente.</p>".to_string()
    } else {
        let mut items = String::new();
        let health_snapshot = health.lock().map(|map| map.clone()).unwrap_or_default();
        for service in services {
            let health_info = health_snapshot
                .get(&service.name)
                .copied()
                .unwrap_or_default();
            let (status_label, status_class) = match health_info.status {
                HealthStatus::Healthy => ("🟢 En línea", "status status--healthy"),
                HealthStatus::Unhealthy => ("🔴 Fuera de servicio", "status status--unhealthy"),
                HealthStatus::Unknown => ("⚪️ Sin datos", "status status--unknown"),
            };
            let last_checked = match health_info.last_checked {
                Some(instant) => {
                    let seconds = instant.elapsed().as_secs();
                    match seconds {
                        0 => "Última verificación hace menos de un segundo".to_string(),
                        1 => "Última verificación hace 1 segundo".to_string(),
                        _ => format!("Última verificación hace {} segundos", seconds),
                    }
                }
                None => "Última verificación pendiente".to_string(),
            };
            items.push_str(&format!(
                "<li><strong>{}</strong><br/><span class=\"{}\">{}</span><br/><span>Prefijo: <code>{}</code></span><br/><span>Base URL: <code>{}</code></span><br/><small>{}</small></li>",
                service.name,
                status_class,
                status_label,
                service.prefix,
                service.base_url,
                last_checked
            ));
        }

        format!("<ul class=\"service-list\">{}</ul>", items)
    };

    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"es\">\n<head>\n    <meta charset=\"utf-8\" />\n    <title>Servicios disponibles</title>\n    <link rel=\"stylesheet\" href=\"https://cdn.jsdelivr.net/npm/water.css@2/out/water.css\" />\n    <style>\n      .service-list {{ list-style: none; padding: 0; }}\n      .service-list li {{ margin-bottom: 1.5rem; }}\n      .status {{ font-weight: bold; display: inline-block; margin-bottom: 0.25rem; }}\n      .status--healthy {{ color: #0a7d24; }}\n      .status--unhealthy {{ color: #c62828; }}\n      .status--unknown {{ color: #616161; }}\n    </style>\n</head>\n<body>\n    <main>\n      <h1>Servicios disponibles</h1>\n        <p>Estos son los servicios registrados actualmente en el runner.</p>\n        {}\n    </main>\n</body>\n</html>\n",
        service_section
    );

    let mut response = Response::from_string(html);
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"text/html; charset=utf-8") {
        response = response.with_header(header);
    }

    response
}

fn start_health_monitor(services: &[Service]) -> SharedHealthMap {
    let health_map: SharedHealthMap = Arc::new(Mutex::new(HashMap::new()));

    if let Ok(mut guard) = health_map.lock() {
        for service in services {
            guard.insert(service.name.clone(), ServiceHealth::default());
        }
    }

    if services.is_empty() {
        return health_map;
    }

    let services_for_monitor = services.to_vec();
    let health_clone = Arc::clone(&health_map);

    thread::spawn(move || loop {
        for service in &services_for_monitor {
            let url = format!("{}/health", service.base_url.trim_end_matches('/'));
            let now = Instant::now();
            let status = match ureq::get(&url)
                .timeout(Duration::from_secs(HEALTH_REQUEST_TIMEOUT_SECS))
                .call()
            {
                Ok(response) if response.status() == 200 => HealthStatus::Healthy,
                Ok(response) => {
                    eprintln!(
                        "Servicio '{}' respondió {} en su healthcheck",
                        service.name,
                        response.status()
                    );
                    HealthStatus::Unhealthy
                }
                Err(error) => {
                    eprintln!(
                        "No se pudo contactar al servicio '{}' en {}: {}",
                        service.name, url, error
                    );
                    HealthStatus::Unhealthy
                }
            };

            if let Ok(mut map) = health_clone.lock() {
                let entry = map.entry(service.name.clone()).or_default();
                entry.status = status;
                entry.last_checked = Some(now);
            }
        }

        thread::sleep(Duration::from_secs(HEALTH_POLL_INTERVAL_SECS));
    });

    health_map
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

fn openapi_path(name: &str) -> PathBuf {
    PathBuf::from("services").join(name).join("openapi.json")
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

    let paths = document
        .get("paths")
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            anyhow!(
                "OpenAPI specification for service '{}' missing 'paths' object",
                name
            )
        })?;

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
        bail!(
            "OpenAPI specification for service '{}' does not declare any GET endpoints",
            name
        );
    }

    Ok(allowed)
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
