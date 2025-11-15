#!/usr/bin/env bash
set -euo pipefail

TARGET="wasm32-wasip1"
SCRIPT_DIR="$(dirname "$0")"
DEFAULT_WASI_SDK="$HOME/.wasmedge/wasi-sdk-24.0"

configure_wasi_sdk() {
    local wasi_sdk="${WASI_SDK_PATH:-$DEFAULT_WASI_SDK}"

    if [ -d "$wasi_sdk" ]; then
        export WASI_SDK_PATH="$wasi_sdk"
        export CC_wasm32_wasip1="$wasi_sdk/bin/clang"
        export AR_wasm32_wasip1="$wasi_sdk/bin/llvm-ar"
    else
        echo "Warning: WASI SDK not found at '$wasi_sdk'. Install it or set WASI_SDK_PATH." >&2
    fi
}

build_module() {
    local module_name="$1"
    local module_dir="services/$module_name"

    if [ ! -d "$module_dir" ]; then
        echo "Module directory '$module_dir' does not exist." >&2
        return 1
    fi

    local manifest_path="$module_dir/Cargo.toml"

    if [ ! -f "$manifest_path" ]; then
        echo "Skipping module '$module_name' because manifest '$manifest_path' was not found." >&2
        return 0
    fi

    RUSTFLAGS="--cfg wasmedge --cfg tokio_unstable" cargo build \
        --target "$TARGET" \
        --release \
        --manifest-path "$manifest_path"

    local wasm_path="$module_dir/target/${TARGET}/release/$module_name.wasm"

    if [ -f "$wasm_path" ]; then
        local output_wasm="$module_dir/$module_name.wasm"
        wasmedge compile "$wasm_path" "$output_wasm"
        echo "Generated WasmEdge module at $output_wasm"
    else
        echo "Failed to build WebAssembly module for $module_name" >&2
        return 1
    fi
}

"$SCRIPT_DIR/check_wasm_toolchain.sh"
configure_wasi_sdk

if [ $# -eq 0 ]; then
    shopt -s nullglob
    modules=()

    for module_dir in services/*/; do
        [ -d "$module_dir" ] || continue
        module_dir="${module_dir%/}"
        modules+=("${module_dir##*/}")
    done

    shopt -u nullglob

    if [ ${#modules[@]} -eq 0 ]; then
        echo "No service modules found under 'services'." >&2
        exit 1
    fi

    IFS=$'\n' modules_sorted=($(printf '%s\n' "${modules[@]}" | sort))
    unset IFS

    for module in "${modules_sorted[@]}"; do
        echo "Building module '$module'..."
        build_module "$module" || exit 1
    done
else
    if [ $# -ne 1 ]; then
        echo "Usage: $0 [<module-name>]" >&2
        exit 1
    fi

    build_module "$1" || exit 1
fi
