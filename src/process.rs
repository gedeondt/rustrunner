use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::{HostOutputStream, StdoutStream, StreamResult, Subscribe, WasiCtxBuilder};

use crate::config::Service;
use crate::logs::{record_log_line, spawn_log_forwarder, SharedLogMap};

const WASM_TARGET: &str = "wasm32-wasip1";
const WASM_PROFILE: &str = "release";

fn module_directory(module_name: &str) -> PathBuf {
    Path::new("services")
        .join(module_name)
        .join("target")
        .join(WASM_TARGET)
        .join(WASM_PROFILE)
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
    if should_use_wasm_services() {
        run_module_with_streams(module_name, None, None)
    } else {
        run_native_service(module_name, None)
    }
}

pub struct ServiceModuleHandle {
    _join: JoinHandle<()>,
}

pub fn start_service_modules(
    services: &[Service],
    logs: &SharedLogMap,
) -> Result<Vec<ServiceModuleHandle>> {
    let mut handles = Vec::new();

    let use_wasm = should_use_wasm_services();

    for service in services {
        let module_name = service.name.clone();
        let handle = if use_wasm {
            let stdout_stream = LogStream::new(&service.name, "stdout", logs);
            let stderr_stream = LogStream::new(&service.name, "stderr", logs);

            thread::Builder::new()
                .name(format!("svc-{}", module_name))
                .spawn(move || {
                    if let Err(error) = run_module_with_streams(
                        &module_name,
                        Some(stdout_stream),
                        Some(stderr_stream),
                    ) {
                        eprintln!("service '{module_name}' exited with error: {error:?}");
                    }
                })
        } else {
            let log_store = Arc::clone(logs);
            thread::Builder::new()
                .name(format!("svc-{}", module_name))
                .spawn(move || {
                    if let Err(error) = run_native_service(&module_name, Some(log_store)) {
                        eprintln!("service '{module_name}' exited with error: {error:#}");
                    }
                })
        }
        .with_context(|| format!("failed to spawn thread for service '{}'", service.name))?;

        handles.push(ServiceModuleHandle { _join: handle });
    }

    Ok(handles)
}

fn should_use_wasm_services() -> bool {
    match env::var("RUNNER_USE_WASM") {
        Ok(value) => matches!(value.as_str(), "1" | "true" | "TRUE"),
        Err(_) => false,
    }
}

fn run_native_service(module_name: &str, logs: Option<SharedLogMap>) -> Result<()> {
    let manifest_path = Path::new("services").join(module_name).join("Cargo.toml");

    if !manifest_path.exists() {
        return Err(anyhow!(
            "Manifest for service '{}' was not found at {}",
            module_name,
            manifest_path.display()
        ));
    }

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest_path);

    if logs.is_some() {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn native process for '{module_name}'"))?;

    if let Some(log_store) = logs {
        if let Some(stdout) = child.stdout.take() {
            spawn_log_forwarder(
                module_name.to_string(),
                stdout,
                "stdout",
                Arc::clone(&log_store),
            );
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_forwarder(
                module_name.to_string(),
                stderr,
                "stderr",
                Arc::clone(&log_store),
            );
        }
    }

    child
        .wait()
        .with_context(|| format!("native process for '{module_name}' failed"))?
        .success()
        .then_some(())
        .ok_or_else(|| anyhow!("native process for '{module_name}' exited with a non-zero status"))
}

fn run_module_with_streams(
    module_name: &str,
    stdout_stream: Option<LogStream>,
    stderr_stream: Option<LogStream>,
) -> Result<()> {
    let wasm_path = module_path(module_name)?;

    let engine = Engine::default();
    let module = Module::from_file(&engine, &wasm_path).with_context(|| {
        format!(
            "failed to load module '{}': {}",
            module_name,
            wasm_path.display()
        )
    })?;

    let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
    preview1::add_to_linker_sync(&mut linker, |ctx: &mut WasiP1Ctx| ctx)
        .context("failed to configure WASI imports")?;

    let precompiled = linker.instantiate_pre(&module).with_context(|| {
        format!(
            "failed to prepare module '{}' for instantiation",
            module_name
        )
    })?;

    let mut wasi_ctx_builder = WasiCtxBuilder::new();
    wasi_ctx_builder.inherit_network();
    wasi_ctx_builder.allow_ip_name_lookup(true);

    match stdout_stream {
        Some(stream) => {
            wasi_ctx_builder.stdout(stream);
        }
        None => {
            wasi_ctx_builder.inherit_stdout();
        }
    }

    match stderr_stream {
        Some(stream) => {
            wasi_ctx_builder.stderr(stream);
        }
        None => {
            wasi_ctx_builder.inherit_stderr();
        }
    }

    let wasi_ctx = wasi_ctx_builder.build_p1();
    let mut store = Store::new(&engine, wasi_ctx);

    let instance = precompiled
        .instantiate(&mut store)
        .with_context(|| format!("failed to instantiate module '{}'", module_name))?;

    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .context("module is missing the '_start' entrypoint")?;

    start
        .call(&mut store, ())
        .with_context(|| format!("module '{}' exited with an error", module_name))
}

#[derive(Clone)]
struct LogStream {
    inner: Arc<LogStreamInner>,
}

impl LogStream {
    fn new(service_name: &str, stream_label: &'static str, logs: &SharedLogMap) -> Self {
        LogStream {
            inner: Arc::new(LogStreamInner {
                service_name: service_name.to_string(),
                stream_label,
                logs: Arc::clone(logs),
                buffer: Mutex::new(String::new()),
            }),
        }
    }
}

impl StdoutStream for LogStream {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(LogWriteStream {
            inner: Arc::clone(&self.inner),
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

struct LogStreamInner {
    service_name: String,
    stream_label: &'static str,
    logs: SharedLogMap,
    buffer: Mutex<String>,
}

impl LogStreamInner {
    fn write_bytes(&self, chunk: &[u8]) {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.push_str(&String::from_utf8_lossy(chunk));

        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            self.emit_line(line);
        }
    }

    fn flush(&self) {
        let mut buffer = self.buffer.lock().unwrap();
        if buffer.is_empty() {
            return;
        }
        let line: String = buffer.drain(..).collect();
        drop(buffer);
        self.emit_line(line);
    }

    fn emit_line(&self, mut line: String) {
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        record_log_line(&self.service_name, &line, self.stream_label, &self.logs);
    }
}

struct LogWriteStream {
    inner: Arc<LogStreamInner>,
}

#[async_trait]
impl Subscribe for LogWriteStream {
    async fn ready(&mut self) {}
}

impl HostOutputStream for LogWriteStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.inner.write_bytes(bytes.as_ref());
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.inner.flush();
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(usize::MAX / 2)
    }
}
