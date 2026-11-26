#!/usr/bin/env bash
# Verify the images and firmware the spatial studio boots are present.
#
# Checks the kernel card (staged kernel + ramdisk), the Debian golden base and
# its ready marker, the Windows install media / UEFI variables / TPM state, and
# the QEMU binaries. Guest OS images are created by the studio's install flow;
# this script only reports what is missing and where it belongs.
#
# Exit status: 0 when every REQUIRED item exists (warnings are allowed),
# 1 when a required item is missing.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DATA_DIR="${POSTCARD_VM_DATA:-${REPO_ROOT}/workspace-prototype/data}"
MASTER_DIR="${DATA_DIR}/master-images"

failures=0
warnings=0

ok() { printf '  ok    %s\n' "$1"; }
warn() { printf '  WARN  %s\n' "$1"; warnings=$((warnings + 1)); }
missing() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

exists() { [[ -e "$1" ]]; }

echo "guest data: ${DATA_DIR}"
echo
echo "kernel card"
if exists "${DATA_DIR}"; then
    ok "data directory"
else
    missing "data directory: run 'make stage-vm'"
fi

staged_kernel="${MASTER_DIR}/postcard-minimal-kernel"
built_kernel="${REPO_ROOT}/target/x86_64-unknown-none/release/postcard-minimal-kernel"
if exists "${staged_kernel}"; then
    ok "kernel image (${staged_kernel})"
    if exists "${built_kernel}" && ! cmp -s "${staged_kernel}" "${built_kernel}"; then
        warn "staged kernel differs from target/: run 'make kernel' to restage"
    fi
elif exists "${built_kernel}"; then
    missing "staged kernel missing: run 'make stage-vm' (found target/)"
else
    missing "kernel image: run 'make kernel'"
fi

staged_ramdisk="${MASTER_DIR}/ramdisk.img"
if exists "${staged_ramdisk}"; then
    ok "ramdisk (${staged_ramdisk})"
elif exists "${REPO_ROOT}/ramdisk.img"; then
    missing "staged ramdisk missing: run 'make stage-vm' (found ramdisk.img)"
else
    missing "ramdisk: run 'make userland'"
fi

echo
echo "host binaries"
qemu_path="${POSTCARD_QEMU:-}"
if [[ -z "${qemu_path}" ]]; then
    qemu_path="$(command -v qemu-system-x86_64 2>/dev/null || true)"
fi
if [[ -n "${qemu_path}" && -x "${qemu_path}" ]]; then
    ok "qemu-system-x86_64 (${qemu_path})"
else
    missing "qemu-system-x86_64: install QEMU or set POSTCARD_QEMU"
fi

qemu_img_path="${POSTCARD_QEMU_IMG:-}"
if [[ -z "${qemu_img_path}" ]]; then
    qemu_img_path="$(command -v qemu-img 2>/dev/null || true)"
fi
if [[ -n "${qemu_img_path}" && -x "${qemu_img_path}" ]]; then
    ok "qemu-img (${qemu_img_path})"
else
    missing "qemu-img: needed to create card overlays (install qemu-img)"
fi

if command -v node >/dev/null 2>&1; then
    ok "node"
else
    missing "node: the studio and its API need it"
fi

echo
echo "Debian golden base (Linux cards)"
base_disk="${POSTCARD_LINUX_BASE:-${MASTER_DIR}/debian-golden.qcow2}"
marker="${DATA_DIR}/debian-golden.ready.json"
if exists "${base_disk}"; then
    ok "golden disk (${base_disk})"
else
    warn "no golden disk yet: install Debian from the studio, then 'Finish base installation'"
fi

if exists "${marker}"; then
    marker_disk="$(node -e '
        const fs = require("node:fs");
        try {
            const marker = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
            if (marker.version !== 1) process.exit(2);
            process.stdout.write(String(marker.baseDiskPath ?? ""));
        } catch {
            process.exit(1);
        }
    ' "${marker}" 2>/dev/null)"
    marker_status=$?
    if [[ ${marker_status} -ne 0 ]]; then
        warn "ready marker is unreadable or not version 1 (${marker})"
    elif [[ "$(cd "$(dirname "${marker_disk}")" 2>/dev/null && pwd)/$(basename "${marker_disk}")" != "$(cd "$(dirname "${base_disk}")" && pwd)/$(basename "${base_disk}")" ]]; then
        warn "ready marker points at ${marker_disk}, not ${base_disk}"
    elif ! exists "${base_disk}"; then
        warn "ready marker exists but its golden disk is missing"
    else
        ok "ready marker matches the configured base"
    fi
elif exists "${base_disk}"; then
    warn "golden disk exists but has no ready marker; boot a Linux card in installer mode to finish it"
fi

iso_path="${POSTCARD_DEBIAN_ISO:-${REPO_ROOT}/archive/guest-vm-setup/debian-13.7.0-amd64-netinst.iso}"
if exists "${iso_path}"; then
    ok "Debian installer ISO (${iso_path})"
else
    warn "no Debian installer ISO at ${iso_path}; only needed to create the base"
fi

echo
echo "Windows cards"
for directory in "${DATA_DIR}/windows/disks" "${DATA_DIR}/windows/firmware" "${DATA_DIR}/windows/tpm"; do
    if exists "${directory}"; then
        ok "${directory#"${DATA_DIR}/"}"
    else
        warn "missing ${directory#"${DATA_DIR}/"} (run 'make stage-vm')"
    fi
done

windows_iso="${POSTCARD_WINDOWS_ISO:-}"
if [[ -n "${windows_iso}" ]] && exists "${windows_iso}"; then
    ok "Windows ISO (${windows_iso})"
else
    warn "no Windows ISO: set POSTCARD_WINDOWS_ISO (media is never bundled)"
fi

vars_template="${DATA_DIR}/windows/firmware/OVMF_VARS_TEMPLATE.fd"
if exists "${vars_template}"; then
    ok "UEFI variables template"
else
    warn "missing OVMF_VARS_TEMPLATE.fd; copy your OVMF vars file there before installing Windows"
fi

ovmf_code="${POSTCARD_WSL_OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.ms.fd}"
ovmf_vars="${POSTCARD_WSL_OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"
if exists "${ovmf_code}" && exists "${ovmf_vars}"; then
    ok "OVMF secure-boot firmware (${ovmf_code})"
else
    warn "OVMF secure-boot firmware not found at ${ovmf_code}; install the OVMF package"
fi

windows_disks=("${DATA_DIR}/windows/disks/"*.qcow2)
if [[ -e "${windows_disks[0]}" ]]; then
    ok "Windows card disk(s): ${#windows_disks[@]}"
else
    warn "no Windows card disk yet; install Windows on a Windows card"
fi

echo
echo "Linux card overlays"
overlays=("${DATA_DIR}/overlays/"*.qcow2)
if exists "${DATA_DIR}/overlays"; then
    ok "overlay directory (${#overlays[@]} card(s))"
else
    warn "missing ${DATA_DIR}/overlays (run 'make stage-vm')"
fi

echo
if [[ ${failures} -gt 0 ]]; then
    echo "${failures} required item(s) missing, ${warnings} warning(s)."
    exit 1
fi
echo "All required images are in place (${warnings} warning(s))."
