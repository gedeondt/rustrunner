use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

const WASM_TARGET: &str = "wasm32-wasip2";
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

    let wasi_ctx = WasiCtxBuilder::new().inherit_stdio().build_p1();
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
