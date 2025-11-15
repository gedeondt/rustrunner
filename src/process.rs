use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use sysinfo::{Pid, System};

use crate::config::{self, Service};
use crate::logs::{spawn_log_forwarder, SharedLogMap};
use crate::memory::{record_memory_usage, reset_memory_entry, SharedMemoryMap};

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
    let memory_page_limit = lookup_memory_page_limit(module_name);
    run_module_with_output(module_name, memory_page_limit, OutputMode::Inherit, None)
}

fn lookup_memory_page_limit(module_name: &str) -> Option<u32> {
    match config::load_services() {
        Ok(services) => services
            .iter()
            .find(|service| service.name == module_name)
            .and_then(|service| service.memory_page_limit()),
        Err(error) => {
            eprintln!(
                "warning: could not read service configuration for '{module_name}': {error:?}"
            );
            None
        }
    }
}

pub struct ServiceModuleHandle {
    _join: JoinHandle<()>,
}

pub fn start_service_modules(
    services: &[Service],
    logs: &SharedLogMap,
    memory: &SharedMemoryMap,
) -> Result<Vec<ServiceModuleHandle>> {
    let mut handles = Vec::new();

    for service in services {
        let module_name = service.name.clone();
        let log_store = Arc::clone(logs);
        let memory_page_limit = service.memory_page_limit();
        let memory_store = Arc::clone(memory);

        let handle = thread::Builder::new()
            .name(format!("svc-{}", module_name))
            .spawn(move || {
                let output = OutputMode::Forward {
                    service_name: module_name.clone(),
                    logs: log_store,
                };

                if let Err(error) = run_module_with_output(
                    &module_name,
                    memory_page_limit,
                    output,
                    Some(memory_store),
                ) {
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

fn run_module_with_output(
    module_name: &str,
    memory_page_limit: Option<u32>,
    output: OutputMode,
    memory_store: Option<SharedMemoryMap>,
) -> Result<()> {
    let wasm_path = module_path(module_name)?;

    let mut command = Command::new("wasmedge");
    if let Some(limit) = memory_page_limit {
        command.arg("--memory-page-limit");
        command.arg(limit.to_string());
    }
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
        OutputMode::Forward { service_name, logs } => {
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            let mut child = command
                .spawn()
                .with_context(|| format!("failed to execute module '{module_name}'"))?;

            if let Some(stdout) = child.stdout.take() {
                spawn_log_forwarder(service_name.clone(), stdout, "stdout", Arc::clone(&logs));
            }

            if let Some(stderr) = child.stderr.take() {
                spawn_log_forwarder(service_name.clone(), stderr, "stderr", logs);
            }

            let (stop_flag, monitor_handle) = if let Some(store) = memory_store {
                let stop_flag = Arc::new(AtomicBool::new(false));
                let handle = spawn_memory_probe(
                    service_name.clone(),
                    child.id(),
                    store,
                    Arc::clone(&stop_flag),
                );
                (Some(stop_flag), Some(handle))
            } else {
                (None, None)
            };

            let status = child
                .wait()
                .with_context(|| format!("failed while waiting for '{module_name}'"))?;

            if let Some(flag) = stop_flag {
                flag.store(true, Ordering::Relaxed);
            }

            if let Some(handle) = monitor_handle {
                let _ = handle.join();
            }

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

fn spawn_memory_probe(
    service_name: String,
    pid: u32,
    store: SharedMemoryMap,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut system = System::new();
        let pid = Pid::from_u32(pid);

        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }

            system.refresh_process(pid);

            if let Some(process) = system.process(pid) {
                let usage_bytes = (process.memory() as u64).saturating_mul(1024);
                record_memory_usage(&store, &service_name, Some(usage_bytes));
            } else {
                reset_memory_entry(&store, &service_name);
                break;
            }

            thread::sleep(Duration::from_secs(2));
        }

        reset_memory_entry(&store, &service_name);
    })
}
