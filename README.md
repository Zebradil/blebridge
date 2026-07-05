# blebridge

Bridges a BLE FTMS treadmill to ANT+ (Garmin watches) and re-advertises as a BLE peripheral for mobile app control.

Forked from [roethigj/blebridge](https://github.com/roethigj/blebridge). This fork adds Docker/Docker Compose support for headless deployment on a Raspberry Pi.

## How it works

Three concurrent threads run in a 250ms loop:

- **BLE Central** — connects to the treadmill's FTMS service and reads speed, distance, incline, cadence
- **ANT+ Transmitter** — re-broadcasts data as an ANT+ Stride & Distance Sensor (device type 124) for Garmin watches
- **BLE Peripheral** — re-advertises as `BLE_Bridge_Treadmill` so a mobile app can still connect and control the treadmill

## Hardware requirements

| Component                 | Purpose                                       |
| ------------------------- | --------------------------------------------- |
| Bluetooth adapter #1      | Connect to treadmill (BLE FTMS central)       |
| Bluetooth adapter #2      | Re-advertise for mobile app (BLE GATT server) |
| ANT+ USB stick            | Transmit stride/distance data to Garmin watch |
| FTMS-compatible treadmill | Data source                                   |

The Raspberry Pi 3B has one onboard Bluetooth adapter (hci0, BT 5.0). A second USB Bluetooth dongle and an ANT+ USB stick (Dynastream ANTUSB2) must be connected.

## Prerequisites

**On the Raspberry Pi:**

- Docker and Docker Compose
- BlueZ running (`systemctl status bluetooth`)
- Two Bluetooth adapters and one ANT+ USB stick connected
- Recommended BlueZ host config (`/etc/bluetooth/main.conf`, then
  `systemctl restart bluetooth`):

  ```ini
  [General]
  # LE-only: dual-mode adapters otherwise trigger a second (BR/EDR) pairing
  # prompt on phones; everything the bridge does is LE.
  ControllerMode = le
  # Let a phone that "forgot" the bridge re-pair without manually wiping the
  # stale bond on the Pi (default "never" makes re-pairing loop forever).
  JustWorksRepairing = always
  ```

- Recommended: auto-restart bluetoothd on crash (BlueZ 5.55 can SEGV on
  disconnects; systemd does not restart it by default):

  ```sh
  sudo mkdir -p /etc/systemd/system/bluetooth.service.d
  printf "[Service]\nRestart=on-failure\nRestartSec=2\n" |
    sudo tee /etc/systemd/system/bluetooth.service.d/restart.conf
  sudo systemctl daemon-reload
  ```

**On the build machine (Rust, recommended):**

- Nix with flakes (provides the toolchain via the repo devshell / direnv)
- Docker with buildx (used only to wrap the prebuilt binary in a scratch image)

No QEMU needed — the Rust cross-compile is static musl, not emulation.

**On the build machine (legacy Python):**

- Docker with buildx
- QEMU binfmt registered for arm64 (see build steps below)

## Deployment target

Docker commands in this README require `DOCKER_HOST` to point at the Pi:

```bash
export DOCKER_HOST=ssh://pi@<pi-host>
```

Set it in your shell, a `.envrc` (with [direnv](https://direnv.net/)), or your CI environment. Without it, commands run against your local Docker daemon.

## Configuration

All configuration is in the source files. Key settings:

| Setting                 | File              | Description                                                                                                                                                                                 |
| ----------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `x = 0`                 | `src/blebridge/__main__.py:14` | Which adapter index is used as BLE peripheral (the other is used for FTMS central). Set to `0` or `1` based on your adapter ordering. Verify with `bluetoothctl list` inside the container. |
| `Device_Number = 12345` | `src/blebridge/antsend.py:13`   | ANT+ device number the Garmin watch pairs with                                                                                                                                              |
| Speed range             | `src/blebridge/ftms.py`         | 1–16 km/h                                                                                                                                                                                   |
| Incline range           | `src/blebridge/ftms.py`         | 0–10%                                                                                                                                                                                       |

## Deployment

### 1. Set up ANT+ udev rules on the Pi

Run the following **on the Pi**:

```bash
sudo tee /etc/udev/rules.d/42-ant-usb-sticks.rules > /dev/null << EOF
SUBSYSTEM=="usb", ATTR{idVendor}=="0fcf", ATTR{idProduct}=="1008", MODE="0666"
SUBSYSTEM=="usb", ATTR{idVendor}=="0fcf", ATTR{idProduct}=="1009", MODE="0666"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=0fcf
```

### 2. Build and deploy the Rust bridge

The Rust bridge is the deployment target (see `docs/adr/0001-rust-rewrite.md`).
Build the image locally and push it to the Pi in one step — no building on the
resource-constrained Pi:

```bash
mise run deploy-rust          # cross-builds blebridge:local, loads it onto the Pi, then `up -d`
```

Under the hood `deploy-rust` runs `build-rust` (nix pkgsCross → static-musl
binary → `blebridge:local` image tar via buildx) and then loads the tar onto the
Pi and starts `compose.rust.yaml`. Select the arch with `TARGET_ARCH` (default
`arm64`, the Pi):

```bash
TARGET_ARCH=amd64 mise run build-rust   # build only, e.g. for an x86_64 test host
```

`build-rust` uses nix pkgsCross, not `cross`, because `cross` breaks inside the
nix devshell. `mise run build-rust-cross` keeps the CI `cross` recipe for a
clean shell.

**End users** (no repo checkout / mise): copy `compose.example.yaml` to the host
and run the published ghcr image directly:

```bash
docker compose -f compose.example.yaml pull && docker compose -f compose.example.yaml up -d
```

### 3. Check logs

```bash
mise run logs
```

## App Endpoint validation

After deploying the Rust bridge, a Linux laptop with Bluetooth can validate the
same BLE behavior you would inspect with nRF Connect:

```bash
nix run .#test-bt
```

The test scans for `BLE_Bridge_Treadmill`, connects to it as a mobile app would,
reads the proxied FTMS range characteristics (`2AD4`, `2AD5`), subscribes to the
FTMS notification characteristics (`2ACD`, `2ADA`, `2AD3`), prints raw payloads
as hex, and fails if no `2ACD` treadmill measurement notifications arrive during
the capture window.

Use it in two passes:

1. With the treadmill off, run `nix run .#test-bt -- --capture-seconds 5 --allow-no-frames`
   and confirm the bridge advertises and connects while idle. No live `2ACD`
   frames are expected.
2. With the treadmill on and walking, run `nix run .#test-bt`; it should print
   repeated `2ACD` hex frames and exit successfully.

Options:

```bash
nix run .#test-bt -- --name My_Bridge_Name       # if BLEBRIDGE_BLE_NAME is set
nix run .#test-bt -- --address AA:BB:CC:DD:EE:FF # skip scan and connect directly
nix run .#test-bt -- --capture-seconds 60        # longer live capture
nix run .#test-bt -- --allow-no-frames           # idle-mode check
```

The script matches by advertised name rather than any FTMS device so it does not
accidentally connect to the physical treadmill when both are visible.

### Legacy Python flow

The Python bridge is retired once Rust is validated end-to-end. Until then:

```bash
docker run --privileged --rm tonistiigi/binfmt --install arm64   # register QEMU for arm64
mise run build        # QEMU-emulated arm64 build -> blebridge.tar
mise run deploy        # docker load + `compose.yaml up -d`
```

Expected output when everything works:

```
scanning
ANT+ Channel is open
FTMS Measurement Device Found! <treadmill name>
Connecting to <treadmill name>
BLE_central connected
```

## Troubleshooting

**Container won't start / exits immediately**

Check logs: `docker compose logs blebridge`

**Bluetooth adapter not found / cannot power adapter**

`NET_ADMIN` capability is required to power BT adapters on and off. If it still fails, try `privileged: true` in `compose.yaml`.

**ANT+ stick not visible in container**

Verify udev rules applied (on the Pi): `ls -la /dev/bus/usb/001/` — the ANT+ device (`0fcf:1008`) should have `rw` for all users.

If a kernel module has claimed the stick, run on the Pi:

```bash
echo "blacklist ant_usb" | sudo tee /etc/modprobe.d/blacklist-ant.conf && sudo reboot
```

**Wrong adapter used for FTMS central**

Check adapter order inside the container:

```bash
docker exec blebridge bluetoothctl list
```

If adapters are in the wrong order, change `x` in `src/blebridge/__main__.py` (0 or 1) and rebuild.

**Treadmill not found (scanning indefinitely)**

- Ensure the treadmill is powered on and advertising FTMS
- Verify no other device is already connected to the treadmill's BLE FTMS service

**D-Bus errors**

Ensure `/var/run/dbus` is mounted (see `compose.yaml`) and BlueZ is running on the Pi:

```bash
systemctl is-active bluetooth
```
