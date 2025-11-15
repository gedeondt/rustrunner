use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Result};

use crate::config::Service;
use crate::logs::{spawn_log_forwarder, SharedLogMap};

fn module_directory(module_name: &str) -> PathBuf {
    Path::new("services").join(module_name)
}

fn module_path(module_name: &str) -> Result<PathBuf> {
    let wasm_path = module_directory(module_name).join(format!("{module_name}.wasm"));

    if !wasm_path.exists() {
        return Err(anyhow!(
            "WebAssembly module '{}' was not found at {}",
            module_name,
            wasm_path.display()
        ));
    }

    wasm_path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize module path for '{}'", module_name))
}

pub fn run_module(module_name: &str) -> Result<()> {
    run_module_with_output(module_name, OutputMode::Inherit)
}

pub struct ServiceModuleHandle {
    _join: JoinHandle<()>,
}

pub fn start_service_modules(
    services: &[Service],
    logs: &SharedLogMap,
) -> Result<Vec<ServiceModuleHandle>> {
    let mut handles = Vec::new();

    for service in services {
        let module_name = service.name.clone();
        let log_store = Arc::clone(logs);

        let handle = thread::Builder::new()
            .name(format!("svc-{}", module_name))
            .spawn(move || {
                let output = OutputMode::Forward {
                    service_name: module_name.clone(),
                    logs: log_store,
                };

                if let Err(error) = run_module_with_output(&module_name, output) {
                    eprintln!("service '{module_name}' exited with error: {error:?}");
                }
            })
            .with_context(|| format!("failed to spawn thread for service '{}'", service.name))?;

        handles.push(ServiceModuleHandle { _join: handle });
    }

    Ok(handles)
}

enum OutputMode {
    Inherit,
    Forward {
        service_name: String,
        logs: SharedLogMap,
    },
}

fn run_module_with_output(module_name: &str, output: OutputMode) -> Result<()> {
    let wasm_path = module_path(module_name)?;

    let mut command = Command::new("wasmedge");
    command.arg(&wasm_path);

    match output {
        OutputMode::Inherit => {
            command.stdout(Stdio::inherit());
            command.stderr(Stdio::inherit());
            let status = command
                .status()
                .with_context(|| format!("failed to execute module '{module_name}'"))?;
            if status.success() {
                Ok(())
            } else {
                Err(anyhow!(
                    "module '{}' exited with non-zero status {status}",
                    module_name
                ))
            }
        }
        OutputMode::Forward {
            service_name,
            logs,
        } => {
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            let mut child = command
                .spawn()
                .with_context(|| format!("failed to execute module '{module_name}'"))?;

            if let Some(stdout) = child.stdout.take() {
                spawn_log_forwarder(
                    service_name.clone(),
                    stdout,
                    "stdout",
                    Arc::clone(&logs),
                );
            }

            if let Some(stderr) = child.stderr.take() {
                spawn_log_forwarder(service_name.clone(), stderr, "stderr", logs);
            }

            let status = child
                .wait()
                .with_context(|| format!("failed while waiting for '{module_name}'"))?;

            if status.success() {
                Ok(())
            } else {
                Err(anyhow!(
                    "module '{}' exited with non-zero status {status}",
                    module_name
                ))
            }
        }
    }
}
