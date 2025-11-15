mod config;
mod health;
mod logs;
mod memory;
mod process;
mod queue;
mod scheduler;
mod server;
mod stats;
mod templates;

pub use config::{load_services, Service, ServiceKind};
pub use health::{HealthStatus, ServiceHealth, SharedHealthMap};
pub use logs::{initialize_log_store, SharedLogMap};
pub use memory::{initialize_memory_store, SharedMemoryMap};
pub use process::run_module;
pub use queue::{initialize_queue_registry, SharedQueueRegistry};
pub use scheduler::{start_webhook_schedulers, SharedScheduleMap};
pub use server::run_server;
pub use stats::{initialize_stats_store, record_http_status, SharedStats};

use anyhow::{bail, Result};
use std::env;
use std::io::Cursor;
use std::sync::Arc;

use health::start_health_monitor;
use logs::spawn_log_forwarder;
use process::start_service_modules;

enum Invocation {
    Runner,
    Module(String),
}

pub fn run() -> Result<()> {
    match detect_invocation()? {
        Invocation::Runner => run_high_level_runner(),
        Invocation::Module(module) => run_module(&module),
    }
}

fn detect_invocation() -> Result<Invocation> {
    let mut args = env::args().skip(1);

    let Some(first) = args.next() else {
        return Ok(Invocation::Runner);
    };

    match first.as_str() {
        "--runner" => Ok(Invocation::Runner),
        "--module" => {
            let Some(name) = args.next() else {
                bail!("Missing module name after '--module'");
            };
            Ok(Invocation::Module(name))
        }
        other if other.starts_with('-') => {
            bail!("Unknown argument '{other}'. Use '--module <name>' to run a WebAssembly module.");
        }
        module => Ok(Invocation::Module(module.to_string())),
    }
}

fn run_high_level_runner() -> Result<()> {
    let services = load_services()?;

    let logs = initialize_log_store(&services);
    seed_log_store(&services, &logs);
    let memory = initialize_memory_store(&services);
    let _service_modules = start_service_modules(&services, &logs, &memory)?;
    let health = start_health_monitor(&services);
    let schedules = start_webhook_schedulers(&services);
    let stats = initialize_stats_store();
    let queues = initialize_queue_registry(&services);

    run_server(
        &services, &health, &logs, &schedules, &stats, &queues, &memory,
    )
}

fn seed_log_store(services: &[Service], logs: &SharedLogMap) {
    for service in services {
        let intro = format!(
            "[INFO] Runner inicializado para el servicio '{}'",
            service.name
        );
        let note = "[INFO] Conecta el reenviador de logs del servicio para ver más detalles";
        let payload = format!("{intro}\n{note}\n");
        let reader = Cursor::new(payload.into_bytes());
        spawn_log_forwarder(service.name.clone(), reader, "info", Arc::clone(logs));
    }
}
