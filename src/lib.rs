mod config;
mod health;
mod logs;
mod process;
mod queue;
mod scheduler;
mod server;
mod stats;
mod templates;

pub use config::{load_services, Service, ServiceKind};
pub use health::{HealthStatus, ServiceHealth, SharedHealthMap};
pub use logs::{initialize_log_store, SharedLogMap};
pub use process::run_module;
pub use queue::{initialize_queue_registry, SharedQueueRegistry};
pub use scheduler::{start_webhook_schedulers, SharedScheduleMap};
pub use server::run_server;
pub use stats::{initialize_stats_store, record_http_status, SharedStats};

use anyhow::{bail, Result};

pub fn run() -> Result<()> {
    bail!("The high-level runner is no longer available. Call `run_module` instead.")
}
