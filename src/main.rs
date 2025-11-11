use std::path::PathBuf;
use std::thread;

use anyhow::{anyhow, bail, Context, Result};
use tiny_http::{Response, Server};
use wasi_common::pipe::WritePipe;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::sync::{add_to_linker, WasiCtxBuilder};

const WASM_TARGET: &str = "wasm32-wasip1";

#[derive(Clone, Copy)]
struct ServiceConfig {
    name: &'static str,
    port: u16,
}

fn main() -> Result<()> {
    let services = [
        ServiceConfig {
            name: "hello_world",
            port: 15000,
        },
        ServiceConfig {
            name: "bye_world",
            port: 15001,
        },
    ];

    let mut handles = Vec::new();

    for service in services {
        let handle = thread::Builder::new()
            .name(service.name.to_owned())
            .spawn(move || run_service(service))
            .with_context(|| format!("failed to spawn service '{}'", service.name))?;
        handles.push(handle);
    }

    for handle in handles {
        handle
            .join()
            .map_err(|err| anyhow!("service thread panicked: {err:?}"))??;
    }

    Ok(())
}

fn run_service(config: ServiceConfig) -> Result<()> {
    let module_path = module_path(config.name);

    if !module_path.exists() {
        bail!(
            "WebAssembly module not found at {}. Build it with scripts/build_wasm_module.sh {}",
            module_path.display(),
            config.name
        );
    }

    let engine = Engine::default();
    let module = Module::from_file(&engine, &module_path).with_context(|| {
        format!(
            "failed to load WebAssembly module '{}' from {}",
            config.name,
            module_path.display()
        )
    })?;

    let address = ("0.0.0.0", config.port);
    let server = Server::http(address).map_err(|error| {
        anyhow!(
            "failed to bind service '{}' to port {}: {}",
            config.name,
            config.port,
            error
        )
    })?;

    println!(
        "Service '{}' listening on http://{}:{}",
        config.name, "0.0.0.0", config.port
    );

    for request in server.incoming_requests() {
        let body = match run_wasm_module(&engine, &module) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("Error executing module '{}': {:#}", config.name, error);
                String::from("internal error")
            }
        };

        let response = Response::from_string(body).with_status_code(200);
        if let Err(error) = request.respond(response) {
            eprintln!(
                "Service '{}' failed to respond to request: {}",
                config.name, error
            );
        }
    }

    Ok(())
}

fn module_path(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "services/{name}/target/{target}/release/{name}.wasm",
        name = name,
        target = WASM_TARGET
    ))
}

fn run_wasm_module(engine: &Engine, module: &Module) -> Result<String> {
    let mut linker = Linker::new(engine);
    add_to_linker(&mut linker, |ctx| ctx)?;

    let stdout = WritePipe::new_in_memory();
    let stdout_reader = stdout.clone();

    let wasi_ctx = WasiCtxBuilder::new()
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
