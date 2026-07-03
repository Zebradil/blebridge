# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**blebridge** bridges a BLE FTMS treadmill to ANT+ Garmin watches and mobile apps simultaneously:
- Connects to treadmill as a BLE client (FTMS service, UUID 1826)
- Re-advertises treadmill as `BLE_Bridge_Treadmill` BLE peripheral for mobile app control
- Transmits treadmill metrics to Garmin via ANT+ Stride & Distance Sensor (device type 124)

Designed for Raspberry Pi 3B+ (arm64), deployed headless via Docker.

## Running Locally

```bash
mise run setup
mise run run
```

Requires two Bluetooth adapters, an ANT+ USB stick, BlueZ, D-Bus, and Python deps from `pyproject.toml`.

## Building and Deploying (Docker)

The Rust bridge is the deployment target (see `docs/adr/0001-rust-rewrite.md`);
the Python flow below it is legacy, kept until one walk session validates Rust.

Rust — build locally and push to the Pi (`DOCKER_HOST` from `.envrc`):

```bash
mise run build-rust        # nix pkgsCross -> static-musl binary -> blebridge:local tar (TARGET_ARCH=arm64|amd64, default arm64)
mise run deploy-rust        # build-rust, then load the tar onto the Pi and `compose -f compose.rust.yaml up -d`
mise run logs
```

- `build-rust` uses **nix pkgsCross**, not `cross`: on a nix-managed host the
  devshell's nix compiler env leaks into `cross`'s build container and breaks it.
  `mise run build-rust-cross` keeps the CI `cross` recipe for a clean shell.
- Deploy paths: `deploy-rust` = locally built `blebridge:local` (dev,
  `compose.rust.yaml`, `pull_policy: never`); `deploy-rust-ghcr` = published
  ghcr image (end users copy `compose.example.yaml`).
- Buildx runs locally (`DOCKER_HOST=''` in the task); load + compose target the
  Pi. No QEMU needed — the cross-compile is static musl, not emulation.

Legacy Python (arm64, QEMU-emulated build):

```bash
docker buildx create --name multiarch --driver docker-container --use || true
mise run build && mise run deploy && mise run logs
```

CI: `.github/workflows/ci.yaml` (Rust checks) and `release.yaml` (tagged
`cross` build + multi-arch ghcr push).

## Architecture

The app runs a **250ms main loop** (`src/blebridge/__main__.py`) coordinating three subsystems, each in its own thread:

### 1. BLE Central (`src/blebridge/ble_central.py` — `BleCentral`)
- Acts as GATT client connecting to the treadmill's FTMS service
- Reads four characteristics: Treadmill Measurement (`2ACD`), Fitness Machine Status (`2ADA`), Training Status (`2AD3`), Control Point (`2AD9`)
- Parses binary FTMS frames with `struct.unpack()` into a 10-element array
- Auto-reconnects on disconnect

### 2. ANT+ Transmitter (`src/blebridge/antsend.py` — `AntSend`)
- Broadcasts treadmill metrics as ANT+ device type 124 (Stride & Distance Sensor) via USB stick
- Sends data pages 80/81 (hardware info) and page 1 (stride count, distance, speed, time)
- Uses `openant` library

### 3. BLE Peripheral (`src/blebridge/ble_peripheral.py` — `FtmsPeripheral`, with service definitions in `src/blebridge/ftms.py`)
- Runs a GATT server advertising as `BLE_Bridge_Treadmill`
- Exposes Device Information (180A) and FTMS (1826) services
- Sends `notify` updates every 250ms (treadmill data) or 75ms (status)

### Data Flow
```
Treadmill (BLE FTMS peripheral)
    ↓  BLE Central reads & parses
BleCentral
    ├→ AntSend → Garmin Watch (ANT+)
    └→ FtmsPeripheral → Mobile Apps (BLE)
         └→ Control point commands → back to treadmill via BLE Central
```

### Threading / Shutdown
- `pill2kill`, `pill2kill2`, `pill2kill3` are `threading.Event` objects signaling shutdown to each thread
- Main loop creates async tasks every 250ms: `update_ble_out()`, `update_ant()`, `move_on()`

### Global State
- `ftms.py` holds characteristic values as module-level variables shared between BLE central callbacks and the peripheral notifier

## Key Configuration (hardcoded in source)

| Setting | File | Purpose |
|---------|------|---------|
| Adapter index `x` | `src/blebridge/__main__.py:15` | Which BT adapter used for BLE peripheral (0 or 1) |
| `Device_Number` | `src/blebridge/antsend.py:13` | ANT+ device ID for Garmin pairing |
| `NETWORK_KEY` | `src/blebridge/antsend.py:11` | ANT+ network key (8-byte array) |
| Speed/incline ranges | `src/blebridge/ftms.py:65-74` | Ranges advertised to mobile apps |
| Loop period | `src/blebridge/__main__.py:115` | 250ms update interval |

## Optional GUI

`src/blebridge/gui2.py` + `src/blebridge/qt_brigde.py` provide a PyQt5 desktop UI for manual treadmill control. Not part of the headless deployment. Run with `mise run gui`.
