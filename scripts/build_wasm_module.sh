#!/usr/bin/env bash
set -euo pipefail

TARGET="wasm32-wasip1"

if [ $# -ne 1 ]; then
    echo "Usage: $0 <module-name>" >&2
    exit 1
fi

MODULE_NAME="$1"
MODULE_DIR="services/$MODULE_NAME"

if [ ! -d "$MODULE_DIR" ]; then
    echo "Module directory '$MODULE_DIR' does not exist." >&2
    exit 1
fi

"$(dirname "$0")/check_wasm_toolchain.sh"

cargo build --target "$TARGET" --release --manifest-path "$MODULE_DIR/Cargo.toml"

WASM_PATH="$MODULE_DIR/target/${TARGET}/release/$MODULE_NAME.wasm"

if [ -f "$WASM_PATH" ]; then
    echo "Built WebAssembly module at $WASM_PATH"
else
    echo "Failed to build WebAssembly module for $MODULE_NAME" >&2
    exit 1
fi
