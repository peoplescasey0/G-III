#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Ensure cargo is in PATH
if ! command -v cargo &>/dev/null; then
    if [[ -f "${HOME}/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "${HOME}/.cargo/env"
    fi
fi

if ! command -v cargo &>/dev/null; then
    echo "Error: cargo not found in PATH or ~/.cargo/env" >&2
    exit 1
fi

export CARGO_INCREMENTAL=0

# Ensure ramdisk exists before compiling kernel
RAMDISK_IMG="${REPO_ROOT}/ramdisk.img"
if [[ ! -f "${RAMDISK_IMG}" ]]; then
    echo "ramdisk.img missing; building userland..."
    "${SCRIPT_DIR}/build_user.sh"
fi

echo "=== Building bare-metal kernel (kernel/) ==="
KERNEL_DIR="${REPO_ROOT}/kernel"
KERNEL_ELF="${REPO_ROOT}/target/x86_64-unknown-none/release/postcard-minimal-kernel"

(
    cd "${KERNEL_DIR}"
    cargo build --release --target x86_64-unknown-none --target-dir "${REPO_ROOT}/target"
)

if [[ ! -f "${KERNEL_ELF}" ]]; then
    echo "Error: Kernel ELF not found at ${KERNEL_ELF}" >&2
    exit 1
fi

# Wrap the 64-bit ELF in an ELF32 container (`objcopy -I elf64-x86-64 -O elf32-i386`).
# Because all physical load addresses (1 MiB+) and `entry32` fit in 32-bit fields,
# the resulting ELF passes BOTH `grub-file --is-x86-multiboot2` AND `grub-file --is-x86-multiboot`,
# AND boots directly via `qemu-system-x86_64 -kernel` without needing GRUB or `grub-pc-bin`!
if command -v objcopy &>/dev/null; then
    cp "${KERNEL_ELF}" "${KERNEL_ELF}.elf64"
    objcopy -I elf64-x86-64 -O elf32-i386 "${KERNEL_ELF}.elf64" "${KERNEL_ELF}"
    cp "${KERNEL_ELF}" "${KERNEL_ELF}.elf32"
    echo "Converted ${KERNEL_ELF} to ELF32 container for direct QEMU -kernel + GRUB Multiboot1/2 boot"
fi

# Verify Multiboot1 & Multiboot2 headers
if command -v grub-file &>/dev/null; then
    if grub-file --is-x86-multiboot2 "${KERNEL_ELF}"; then
        echo "Multiboot2 header verified for ${KERNEL_ELF}"
    else
        echo "Error: ${KERNEL_ELF} is not a valid Multiboot2 ELF" >&2
        exit 1
    fi
    if grub-file --is-x86-multiboot "${KERNEL_ELF}"; then
        echo "Multiboot1 header verified for ${KERNEL_ELF} (QEMU -kernel ready)"
    fi
fi

echo "Kernel build complete: ${KERNEL_ELF}"

# The studio boots from workspace-prototype/data/master-images, not target/,
# so stage the fresh image here instead of leaving a stale copy behind.
if [[ -x "${SCRIPT_DIR}/stage_vm_images.sh" ]]; then
    "${SCRIPT_DIR}/stage_vm_images.sh" --quiet
fi
