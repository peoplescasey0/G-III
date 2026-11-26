#!/usr/bin/env bash
# Host-side unit tests for the Ring-0 spatial hypervisor core.
#
# `kernel/src/spatial_wm.rs` is dependency-free apart from `core` (the kernel
# globals are `#[cfg(not(test))]`), so it can be compiled as its own test crate
# with the host target. This runs the 3D math, ray-plane hit testing, focus
# stack and syscall-descriptor tests without QEMU or a cross toolchain.
set -euo pipefail

if ! command -v rustc &>/dev/null && [[ -d "${HOME}/.cargo/bin" ]]; then
    export PATH="${HOME}/.cargo/bin:${PATH}"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SOURCE="${REPO_ROOT}/kernel/src/spatial_wm.rs"
OUTPUT_DIR="${REPO_ROOT}/target/host-tests"
OUTPUT="${OUTPUT_DIR}/spatial_wm"

if ! command -v rustc &>/dev/null; then
    if [[ -f "${HOME}/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "${HOME}/.cargo/env"
    fi
fi

if ! command -v rustc &>/dev/null; then
    echo "Error: rustc not found in PATH or ~/.cargo/env" >&2
    exit 1
fi

if [[ ! -f "${SOURCE}" ]]; then
    echo "Error: ${SOURCE} not found" >&2
    exit 1
fi

mkdir -p "${OUTPUT_DIR}"

echo "=== Building host test binary for spatial_wm ==="
# `--test` enables `cfg(test)`, which excludes the kernel-only global workspace.
rustc \
    --edition 2021 \
    --test \
    -C opt-level=1 \
    -o "${OUTPUT}" \
    "${SOURCE}"

echo "=== Running spatial hypervisor unit tests ==="
"${OUTPUT}" --test-threads=1
