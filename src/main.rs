use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::sync::{add_to_linker, WasiCtxBuilder};

const WASM_TARGET: &str = "wasm32-wasip1";

fn main() -> Result<()> {
    let module_path = PathBuf::from(format!(
        "services/hello_world/target/{target}/release/hello_world.wasm",
        target = WASM_TARGET
    ));

    if !module_path.exists() {
        bail!(
            "WebAssembly module not found at {}. Build it with scripts/build_wasm_module.sh hello_world",
            module_path.display()
        );
    }

    run_wasm_module(&module_path)
        .with_context(|| format!("failed to run WebAssembly module at {}", module_path.display()))
}

fn run_wasm_module(path: &Path) -> Result<()> {
    let engine = Engine::default();
    let module = Module::from_file(&engine, path)?;
    let mut linker = Linker::new(&engine);

    add_to_linker(&mut linker, |ctx| ctx)?;

    let wasi_ctx = WasiCtxBuilder::new()
        .inherit_stdout()
        .inherit_stderr()
        .build();

    let mut store = Store::new(&engine, wasi_ctx);
    let instance = linker.instantiate(&mut store, &module)?;
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .context("`_start` function not found in module")?;
    start.call(&mut store, ())?;
    Ok(())
}
