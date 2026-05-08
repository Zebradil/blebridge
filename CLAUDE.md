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
python blebridge.py
```

Requires two Bluetooth adapters, an ANT+ USB stick, BlueZ, D-Bus, and Python deps from `Dockerfile`.

## Building and Deploying (Docker, arm64)

```bash
# Build for Raspberry Pi
docker buildx create --name multiarch --driver docker-container --use
docker buildx build --platform linux/arm64 -t blebridge:latest --output type=docker,dest=blebridge.tar .

# Deploy: requires DOCKER_HOST set (e.g. via .envrc) to point at the Pi
docker load -i blebridge.tar
docker compose up -d
docker compose logs -f
```

No automated tests or linting pipeline exists.

## Architecture

The app runs a **250ms main loop** (`blebridge.py`) coordinating three subsystems, each in its own thread:

### 1. BLE Central (`ble_central.py` — `BleCentral`)
- Acts as GATT client connecting to the treadmill's FTMS service
- Reads four characteristics: Treadmill Measurement (`2ACD`), Fitness Machine Status (`2ADA`), Training Status (`2AD3`), Control Point (`2AD9`)
- Parses binary FTMS frames with `struct.unpack()` into a 10-element array
- Auto-reconnects on disconnect

### 2. ANT+ Transmitter (`antsend.py` — `AntSend`)
- Broadcasts treadmill metrics as ANT+ device type 124 (Stride & Distance Sensor) via USB stick
- Sends data pages 80/81 (hardware info) and page 1 (stride count, distance, speed, time)
- Uses `openant` library

### 3. BLE Peripheral (`ble_peripheral.py` — `FtmsPeripheral`, with service definitions in `ftms.py`)
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
| Adapter index `x` | `blebridge.py:15` | Which BT adapter used for BLE peripheral (0 or 1) |
| `Device_Number` | `antsend.py:13` | ANT+ device ID for Garmin pairing |
| `NETWORK_KEY` | `antsend.py:11` | ANT+ network key (8-byte array) |
| Speed/incline ranges | `ftms.py:65-74` | Ranges advertised to mobile apps |
| Loop period | `blebridge.py:115` | 250ms update interval |

## Optional GUI

`gui2.py` + `qt_brigde.py` provide a PyQt5 desktop UI for manual treadmill control. Not part of the headless deployment.
