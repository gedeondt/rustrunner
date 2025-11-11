mod config;
mod health;
mod logs;
mod process;
mod scheduler;
mod server;
mod stats;

pub use config::{load_services, Service};
pub use health::{HealthStatus, ServiceHealth, SharedHealthMap};
pub use logs::{initialize_log_store, SharedLogMap};
pub use process::start_service_processes;
pub use scheduler::{start_webhook_schedulers, SharedScheduleMap};
pub use server::run_server;
pub use stats::{initialize_stats_store, record_http_status, SharedStats};

use anyhow::Result;

pub fn run() -> Result<()> {
    let services = load_services()?;
    let logs = initialize_log_store(&services);
    let _service_guards = start_service_processes(&services, &logs)?;
    let health = health::start_health_monitor(&services);
    let schedules = scheduler::start_webhook_schedulers(&services);

    server::run_server(&services, &health, &logs, &schedules)
    let stats = stats::initialize_stats_store();

    server::run_server(&services, &health, &logs, &stats)
}
