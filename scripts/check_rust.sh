#!/usr/bin/env bash
set -u

REQUIRED_VERSION="1.70.0"

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

compare_versions() {
    # returns 0 if version >= required
    local current=$1
    local required=$2
    if [[ "$current" == "$required" ]]; then
        return 0
    fi

    # Split versions into arrays
    IFS='.' read -r -a current_parts <<<"$current"
    IFS='.' read -r -a required_parts <<<"$required"

    # Normalize lengths by padding with zeros
    local len=${#required_parts[@]}
    if [[ ${#current_parts[@]} -lt $len ]]; then
        for ((i=${#current_parts[@]}; i<len; i++)); do
            current_parts[i]=0
        done
    fi

    for ((i=0; i<len; i++)); do
        local c=${current_parts[i]:-0}
        local r=${required_parts[i]:-0}
        if (( c > r )); then
            return 0
        elif (( c < r )); then
            return 1
        fi
    done

    return 0
}

print_install_instructions() {
    local os=$1
    cat <<INSTRUCTIONS
Rust ${REQUIRED_VERSION} or newer is required.

Installation instructions for ${os}:
- Using rustup (recommended):
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

After installation, restart your shell and re-run this script.
INSTRUCTIONS

    if [[ "$os" == "macOS" ]]; then
        cat <<'BREW'
If you prefer Homebrew, install rustup-init and follow the prompts:
  brew install rustup-init
  rustup-init
BREW
    elif [[ "$os" == "Linux" ]]; then
        cat <<'LINUX'
If your distribution provides Rust packages, ensure they are recent enough or use rustup for the latest stable toolchain.
LINUX
    fi
}

main() {
    local os="Unknown"
    case "$(uname -s)" in
        Darwin)
            os="macOS"
            ;;
        Linux)
            os="Linux"
            ;;
    esac

    if ! command_exists rustc; then
        echo "rustc is not installed."
        print_install_instructions "$os"
        exit 1
    fi

    local version
    version=$(rustc --version 2>/dev/null | awk '{print $2}')

    if compare_versions "$version" "$REQUIRED_VERSION"; then
        echo "Found rustc $version (meets requirement of >= ${REQUIRED_VERSION})."
        exit 0
    else
        echo "Found rustc $version, which is older than required ${REQUIRED_VERSION}."
        print_install_instructions "$os"
        exit 1
    fi
}

main "$@"
