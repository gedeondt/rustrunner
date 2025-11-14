#!/usr/bin/env bash
set -euo pipefail

TARGET="wasm32-wasi"

if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup is required to manage Rust targets." >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is required to build WebAssembly modules." >&2
    exit 1
fi

if rustup target list --installed | grep -q "^${TARGET}$"; then
    echo "${TARGET} target is installed."
else
    echo "${TARGET} target is not installed. Install it with:" >&2
    echo "  rustup target add ${TARGET}" >&2
    exit 1
fi
