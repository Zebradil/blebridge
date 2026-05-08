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

**On the build machine:**

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
| `x = 0`                 | `blebridge.py:14` | Which adapter index is used as BLE peripheral (the other is used for FTMS central). Set to `0` or `1` based on your adapter ordering. Verify with `bluetoothctl list` inside the container. |
| `Device_Number = 12345` | `antsend.py:13`   | ANT+ device number the Garmin watch pairs with                                                                                                                                              |
| Speed range             | `ftms.py`         | 1–16 km/h                                                                                                                                                                                   |
| Incline range           | `ftms.py`         | 0–10%                                                                                                                                                                                       |

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

### 2. Register QEMU on the build machine

The Pi is arm64 but the build machine may be x86_64. Register QEMU to enable cross-compilation:

```bash
docker run --privileged --rm tonistiigi/binfmt --install arm64
```

### 3. Build the arm64 image

```bash
# Create a builder that supports cross-compilation
docker buildx create --name multiarch --driver docker-container --use

# Build and export to a tar file
docker buildx build --platform linux/arm64 -t blebridge:latest \
  --output type=docker,dest=blebridge.tar .
```

### 4. Load the image on the Pi

```bash
docker load -i blebridge.tar
```

### 5. Start the service

Run from the cloned repo directory — `docker compose` uses the local `compose.yaml` and `DOCKER_HOST` routes commands to the Pi:

```bash
cd /path/to/blebridge
docker compose up -d
```

### 6. Check logs

```bash
docker compose logs -f
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

If adapters are in the wrong order, change `x` in `blebridge.py` (0 or 1) and rebuild.

**Treadmill not found (scanning indefinitely)**

- Ensure the treadmill is powered on and advertising FTMS
- Verify no other device is already connected to the treadmill's BLE FTMS service

**D-Bus errors**

Ensure `/var/run/dbus` is mounted (see `compose.yaml`) and BlueZ is running on the Pi:

```bash
systemctl is-active bluetooth
```
