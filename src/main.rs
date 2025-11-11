use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tiny_http::{Request, Response, Server};
use wasi_common::pipe::WritePipe;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::sync::{add_to_linker, WasiCtxBuilder};

const WASM_TARGET: &str = "wasm32-wasip1";
const ENTRY_PORT: u16 = 14000;
const SERVICE_NAMES: &[&str] = &["hello_world", "bye_world"];

struct Service {
    name: &'static str,
    prefix: String,
    module: Module,
}

#[derive(Deserialize)]
struct RawServiceConfig {
    prefix: String,
}

fn main() -> Result<()> {
    let engine = Engine::default();
    let services = load_services(&engine)?;

    let server = Server::http(("0.0.0.0", ENTRY_PORT)).map_err(|error| {
        anyhow!(
            "failed to bind entrypoint to port {}: {}",
            ENTRY_PORT,
            error
        )
    })?;

    println!("Runner listening on http://{}:{}", "0.0.0.0", ENTRY_PORT);

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(&engine, &services, request) {
            eprintln!("Failed to handle request: {:#}", error);
        }
    }

    Ok(())
}

fn load_services(engine: &Engine) -> Result<Vec<Service>> {
    let mut services = Vec::new();

    for &name in SERVICE_NAMES {
        let prefix = read_service_prefix(name)?;
        let module_path = module_path(name);

        if !module_path.exists() {
            bail!(
                "WebAssembly module not found at {}. Build it with scripts/build_wasm_module.sh {}",
                module_path.display(),
                name
            );
        }

        let module = Module::from_file(engine, &module_path).with_context(|| {
            format!(
                "failed to load WebAssembly module '{}' from {}",
                name,
                module_path.display()
            )
        })?;

        services.push(Service {
            name,
            prefix,
            module,
        });
    }

    Ok(services)
}

fn handle_request(engine: &Engine, services: &[Service], request: Request) -> Result<()> {
    let path = request.url().split('?').next().unwrap_or("");
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

    let args = [service.name, endpoint];

    let response = match run_wasm_module(engine, &service.module, &args) {
        Ok(body) => Response::from_string(body).with_status_code(200),
        Err(error) => {
            eprintln!("Error executing module '{}': {:#}", service.name, error);
            Response::from_string("internal error").with_status_code(500)
        }
    };

    request.respond(response)?;
    Ok(())
}

fn run_wasm_module(engine: &Engine, module: &Module, args: &[&str]) -> Result<String> {
    let mut linker = Linker::new(engine);
    add_to_linker(&mut linker, |ctx| ctx)?;

    let stdout = WritePipe::new_in_memory();
    let stdout_reader = stdout.clone();

    let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

    let wasi_ctx = WasiCtxBuilder::new()
        .args(&argv)?
        .stdout(Box::new(stdout))
        .inherit_stderr()
        .build();

    let mut store = Store::new(engine, wasi_ctx);
    let instance = linker.instantiate(&mut store, module)?;
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .context("`_start` function not found in module")?;
    start.call(&mut store, ())?;
    drop(store);

    let bytes = stdout_reader
        .try_into_inner()
        .map_err(|_| anyhow!("failed to read stdout from module"))?
        .into_inner();
    let output = String::from_utf8(bytes)?;
    Ok(output.trim().to_owned())
}

fn module_path(name: &str) -> PathBuf {
    PathBuf::from("services")
        .join(name)
        .join("target")
        .join(WASM_TARGET)
        .join("release")
        .join(format!("{name}.wasm"))
}

fn config_path(name: &str) -> PathBuf {
    PathBuf::from("services")
        .join(name)
        .join("config")
        .join("service.json")
}

fn read_service_prefix(name: &str) -> Result<String> {
    let path = config_path(name);
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read configuration for service '{}' at {}",
            name,
            path.display()
        )
    })?;

    let RawServiceConfig { prefix } = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse configuration for service '{}' at {}",
            name,
            path.display()
        )
    })?;

    if prefix.trim().is_empty() {
        bail!("prefix for service '{}' cannot be empty", name);
    }

    Ok(prefix)
}
