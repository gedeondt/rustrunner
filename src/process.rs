use std::fs;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use toml::Value;

use crate::config::{service_manifest_path, Service};
use crate::logs::{spawn_log_forwarder, SharedLogMap};

const SERVICE_STARTUP_ATTEMPTS: usize = 50;
const SERVICE_STARTUP_BACKOFF_MS: u64 = 100;

#[cfg(unix)]
fn configure_memory_limit(command: &mut Command, limit_bytes: u64) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(move || {
            set_memory_limit(limit_bytes)?;
            Ok(())
        });
    }
}

#[cfg(unix)]
fn set_memory_limit(limit_bytes: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: limit_bytes as libc::rlim_t,
        rlim_max: limit_bytes as libc::rlim_t,
    };

    let result = unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn configure_memory_limit(_command: &mut Command, _limit_bytes: u64) {}

fn build_service(manifest_path: &Path, service_name: &str) -> Result<()> {
    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest_path)
        .status()
        .with_context(|| format!("failed to build service '{}' via cargo", service_name))?;

    if status.success() {
        Ok(())
    } else {
        bail!("cargo build failed for service '{}'", service_name);
    }
}

fn service_binary_path(manifest_path: &Path) -> Result<PathBuf> {
    let service_dir = manifest_path.parent().ok_or_else(|| {
        anyhow!(
            "service manifest '{}' does not have a parent directory",
            manifest_path.display()
        )
    })?;

    let package_name = package_name_from_manifest(manifest_path)?;

    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut relative_path = PathBuf::from("target").join("debug").join(package_name);

    #[cfg(windows)]
    {
        relative_path.set_extension("exe");
    }

    let candidate = service_dir.join(&relative_path);

    if !candidate.exists() {
        bail!(
            "compiled binary for manifest '{}' was not found at {}",
            manifest_path.display(),
            candidate.display()
        );
    }

    let binary_path = candidate.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize binary path for manifest '{}'",
            manifest_path.display()
        )
    })?;

    Ok(binary_path)
}

fn package_name_from_manifest(manifest_path: &Path) -> Result<String> {
    let contents = fs::read_to_string(manifest_path).with_context(|| {
        format!(
            "failed to read service manifest at {}",
            manifest_path.display()
        )
    })?;

    let document: Value = toml::from_str(&contents).with_context(|| {
        format!(
            "failed to parse service manifest at {}",
            manifest_path.display()
        )
    })?;

    let package = document
        .get("package")
        .and_then(Value::as_table)
        .ok_or_else(|| {
            anyhow!(
                "service manifest at '{}' is missing the [package] section",
                manifest_path.display()
            )
        })?;

    let name = package.get("name").and_then(Value::as_str).ok_or_else(|| {
        anyhow!(
            "service manifest at '{}' is missing package.name",
            manifest_path.display()
        )
    })?;

    Ok(name.to_owned())
}

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
        let memory_limit_mib = service.memory_limit_bytes / (1024 * 1024);
        println!(
            "Starting service '{}' using manifest {} (memory limit: {} MiB)",
            service.name,
            manifest_path.display(),
            memory_limit_mib
        );

        build_service(&manifest_path, &service.name)?;

        let binary_path = service_binary_path(&manifest_path).with_context(|| {
            format!(
                "failed to resolve executable for service '{}'",
                service.name
            )
        })?;

        let service_dir = manifest_path.parent().ok_or_else(|| {
            anyhow!(
                "service manifest '{}' does not have a parent directory",
                manifest_path.display()
            )
        })?;

        let mut command = Command::new(&binary_path);
        command
            .current_dir(service_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        configure_memory_limit(&mut command, service.memory_limit_bytes);

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start service '{}' using binary {}",
                service.name,
                binary_path.display()
            )
        })?;

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
            memory_limit_bytes: 64 * 1024 * 1024,
            allowed_get_endpoints: Default::default(),
        };

        let path = crate::config::service_manifest_path(&service.name);
        assert!(path.ends_with("svc/Cargo.toml"));
    }
}
