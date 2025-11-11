use std::net::ToSocketAddrs;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::config::{service_manifest_path, Service};
use crate::logs::{spawn_log_forwarder, SharedLogMap};

const SERVICE_STARTUP_ATTEMPTS: usize = 50;
const SERVICE_STARTUP_BACKOFF_MS: u64 = 100;

pub struct ServiceGuard {
    name: String,
    child: Child,
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        use std::io::ErrorKind;

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

pub fn start_service_processes(
    services: &[Service],
    logs: &SharedLogMap,
) -> Result<Vec<ServiceGuard>> {
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
            spawn_log_forwarder(service.name.clone(), stdout, "stdout", Arc::clone(logs));
        }

        if let Some(stderr) = child.stderr.take() {
            spawn_log_forwarder(service.name.clone(), stderr, "stderr", Arc::clone(logs));
        }

        match wait_for_service(service) {
            Ok(()) => {
                guards.push(ServiceGuard {
                    name: service.name.clone(),
                    child,
                });
            }
            Err(error) => {
                use std::io::ErrorKind;

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
    use std::net::TcpStream;

    let url = url::Url::parse(&service.base_url).with_context(|| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_path_uses_service_name() {
        let service = Service {
            name: "svc".into(),
            prefix: "svc".into(),
            base_url: "http://localhost:1234".into(),
            allowed_get_endpoints: Default::default(),
        };

        let path = crate::config::service_manifest_path(&service.name);
        assert!(path.ends_with("svc/Cargo.toml"));
    }
}
