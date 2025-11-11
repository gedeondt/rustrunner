mod config;
mod health;
mod logs;
mod process;
mod server;

pub use config::{load_services, Service};
pub use health::{HealthStatus, ServiceHealth, SharedHealthMap};
pub use logs::{initialize_log_store, SharedLogMap};
pub use process::start_service_processes;
pub use server::run_server;

use anyhow::Result;

pub fn run() -> Result<()> {
    let services = load_services()?;
    let logs = initialize_log_store(&services);
    let _service_guards = start_service_processes(&services, &logs)?;
    let health = health::start_health_monitor(&services);

    server::run_server(&services, &health, &logs)
}
