#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

ISO_PATH="${REPO_ROOT}/target/postcard-spatial-os.iso"
DISK_PATH="${REPO_ROOT}/target/postcard-os.raw"
KERNEL_ELF="${REPO_ROOT}/target/x86_64-unknown-none/release/postcard-minimal-kernel"

BOOT_MODE="auto" # auto, iso, disk, kernel
MEMORY="256M"
DISPLAY_OPT=""
SERIAL_TTY="${SERIAL_TTY:-}"
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --kernel|-k)
            BOOT_MODE="kernel"
            shift
            ;;
        --tty|--serial-tty)
            SERIAL_TTY="$2"
            shift 2
            ;;
        --iso)
            BOOT_MODE="iso"
            shift
            ;;
        --disk)
            BOOT_MODE="disk"
            if [[ $# -gt 1 && ! "$2" =~ ^-- ]]; then
                DISK_PATH="$2"
                shift
            fi
            shift
            ;;
        --headless)
            DISPLAY_OPT="-display none"
            shift
            ;;
        -m|--memory)
            MEMORY="$2"
            shift 2
            ;;
        *)
            EXTRA_ARGS+=("$1")
            shift
            ;;
    esac
done

if [[ "$BOOT_MODE" == "auto" ]]; then
    # If grub-pc-bin (/usr/lib/grub/i386-pc) is not installed, grub-mkrescue ISOs lack
    # legacy BIOS El Torito boot records and fail in SeaBIOS QEMU (`Could not read from CDROM`).
    # Prefer direct `qemu-system-x86_64 -kernel` boot in that case.
    if [[ ! -d "/usr/lib/grub/i386-pc" ]]; then
        BOOT_MODE="kernel"
    elif [[ -f "$ISO_PATH" ]]; then
        BOOT_MODE="iso"
    elif [[ -f "$DISK_PATH" ]]; then
        BOOT_MODE="disk"
    elif [[ -f "$KERNEL_ELF" ]]; then
        BOOT_MODE="kernel"
    elif command -v grub-mkrescue &>/dev/null; then
        echo "No boot media found. Building ISO..."
        "${SCRIPT_DIR}/create_iso.sh"
        BOOT_MODE="iso"
    else
        echo "Building ELF32-wrapped kernel for direct QEMU -kernel boot..."
        "${SCRIPT_DIR}/build_kernel.sh"
        BOOT_MODE="kernel"
    fi
fi

if [[ -n "$SERIAL_TTY" ]]; then
    echo "Routing COM1 (0x3F8) Umbilical Cord directly to physical UART: $SERIAL_TTY (115200 8N1)"
    stty -F "$SERIAL_TTY" 115200 cs8 -cstopb -parenb raw -echo 2>/dev/null || true
    QEMU_CMD=(qemu-system-x86_64 -name "Post Card OS" -machine pc -m "$MEMORY" -chardev "serial,id=com1,path=${SERIAL_TTY}" -serial chardev:com1 -no-reboot)
else
    QEMU_CMD=(qemu-system-x86_64 -name "Post Card OS" -machine pc -m "$MEMORY" -serial mon:stdio -no-reboot)
fi

if [[ -n "$DISPLAY_OPT" ]]; then
    QEMU_CMD+=($DISPLAY_OPT)
fi

if [[ "$BOOT_MODE" == "kernel" ]]; then
    if [[ ! -f "$KERNEL_ELF" ]]; then
        "${SCRIPT_DIR}/build_kernel.sh"
    fi
    echo "Booting Post Card OS directly via QEMU -kernel ($KERNEL_ELF)..."
    QEMU_CMD+=(-kernel "$KERNEL_ELF")
elif [[ "$BOOT_MODE" == "iso" ]]; then
    if [[ ! -f "$ISO_PATH" ]]; then
        echo "Error: ISO not found at $ISO_PATH. Run scripts/create_iso.sh first." >&2
        exit 1
    fi
    echo "Booting Post Card OS from ISO ($ISO_PATH)..."
    QEMU_CMD+=(-cdrom "$ISO_PATH" -boot d)
else
    if [[ ! -f "$DISK_PATH" ]]; then
        echo "Error: Disk image not found at $DISK_PATH." >&2
        exit 1
    fi
    echo "Booting Post Card OS from raw disk ($DISK_PATH)..."
    QEMU_CMD+=(-drive "file=$DISK_PATH,format=raw,if=ide,index=0" -boot order=c)
fi

if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
    QEMU_CMD+=("${EXTRA_ARGS[@]}")
fi

exec "${QEMU_CMD[@]}"
