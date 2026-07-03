#!/usr/bin/env python3
"""Capture raw FTMS frames from a real treadmill into a JSONL fixture file.

Connects to the first BLE device advertising the FTMS service (0x1826),
subscribes to Treadmill Measurement (0x2ACD) notifications, and logs every
raw frame with a monotonic timestamp. Also reads Supported Speed Range
(0x2AD4) and Supported Inclination Range (0x2AD5) once and records their raw
bytes (or their absence) in a header record.

Operator annotations: whenever the treadmill console display changes, type
the shown speed (e.g. "3.5") and press Enter. Frames captured after that
inherit the annotation until the next one. Type "q" to finish.

Output format (JSONL, one JSON object per line) — documented in
tests/fixtures/ftms/README.md:

  {"type": "header", "captured_at": "...", "device": "...", "address": "...",
   "supported_speed_range_2ad4": "<hex or null>",
   "supported_inclination_range_2ad5": "<hex or null>"}
  {"type": "annotation", "t": <monotonic s>, "speed": "3.5"}
  {"type": "frame", "t": <monotonic s>, "hex": "<raw 2ACD payload>",
   "speed_annotation": "3.5" | null}

Usage (on the Pi, with bluezero installed and the treadmill powered on):

  python3 tools/capture_ftms.py --out tests/fixtures/ftms/session-YYYYMMDD.jsonl
"""

import argparse
import datetime
import json
import sys
import threading
import time

FTMS_SRV = '00001826-0000-1000-8000-00805f9b34fb'
TM_DATA_UUID = '00002acd-0000-1000-8000-00805f9b34fb'          # Treadmill Measurement
SPEED_RANGE_UUID = '00002ad4-0000-1000-8000-00805f9b34fb'      # Supported Speed Range
INCLINE_RANGE_UUID = '00002ad5-0000-1000-8000-00805f9b34fb'    # Supported Inclination Range


def scan_for_ftms(adapter_address, timeout):
    """Return (address, alias) of the first device advertising FTMS, or None."""
    from bluezero import adapter, constants, dbus_tools

    dongle = adapter.Adapter(adapter_addr=adapter_address)
    if not dongle.powered:
        dongle.powered = True
    print(f"Scanning for FTMS devices on {dongle.address} ({timeout:.0f}s)...")
    dongle.nearby_discovery(timeout=timeout)

    managed = dbus_tools.get_managed_objects()
    adapter_path = None
    for path, ifaces in managed.items():
        iface = ifaces.get(constants.ADAPTER_INTERFACE)
        if iface and str(iface.get('Address', '')).upper() == dongle.address.upper():
            adapter_path = path
            break
    if adapter_path is None:
        return None

    for path, ifaces in managed.items():
        dev = ifaces.get(constants.DEVICE_INTERFACE)
        if dev is None or not path.startswith(adapter_path + '/'):
            continue
        uuids = [str(u).lower() for u in dev.get('UUIDs', [])]
        if FTMS_SRV in uuids:
            return str(dev.get('Address', '')), str(dev.get('Alias', ''))
    return None


def read_optional_char(char):
    """Read a characteristic's raw bytes, or None if absent/unreadable."""
    try:
        value = char.value
        if value is None:
            return None
        return bytes(value)
    except Exception as e:  # ponytail: absence is data, any failure counts as absent
        print(f"  (could not read: {type(e).__name__}: {e})")
        return None


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--adapter', default=None,
                    help='BT adapter MAC address (default: first adapter)')
    ap.add_argument('--out', default=None,
                    help='output JSONL file (default: ftms-capture-<timestamp>.jsonl)')
    ap.add_argument('--scan-timeout', type=float, default=15.0,
                    help='scan duration in seconds (default: 15)')
    args = ap.parse_args()

    from bluezero import adapter, central

    adapter_address = args.adapter
    if adapter_address is None:
        adapter_address = list(adapter.Adapter.available())[0].address

    found = scan_for_ftms(adapter_address, args.scan_timeout)
    if not found:
        print("No FTMS device found. Is the treadmill powered on and in range?")
        sys.exit(1)
    dev_address, dev_alias = found
    print(f"Found: {dev_alias} ({dev_address})")

    out_path = args.out or f"ftms-capture-{datetime.datetime.now():%Y%m%d-%H%M%S}.jsonl"
    monitor = central.Central(adapter_addr=adapter_address, device_addr=dev_address)
    tm_char = monitor.add_characteristic(FTMS_SRV, TM_DATA_UUID)
    speed_range_char = monitor.add_characteristic(FTMS_SRV, SPEED_RANGE_UUID)
    incline_range_char = monitor.add_characteristic(FTMS_SRV, INCLINE_RANGE_UUID)

    print("Connecting...")
    monitor.connect()
    for _ in range(10):
        if monitor.connected:
            break
        time.sleep(3)
        if not monitor.connected:
            monitor.connect()
    if not monitor.connected:
        print("Could not connect.")
        sys.exit(1)
    print("Connected.")

    print("Reading Supported Speed Range (2AD4)...")
    speed_range = read_optional_char(speed_range_char)
    print(f"  2AD4: {speed_range.hex() if speed_range else 'ABSENT'}")
    print("Reading Supported Inclination Range (2AD5)...")
    incline_range = read_optional_char(incline_range_char)
    print(f"  2AD5: {incline_range.hex() if incline_range else 'ABSENT'}")

    out = open(out_path, 'w')
    lock = threading.Lock()

    def write_record(rec):
        with lock:
            out.write(json.dumps(rec) + '\n')
            out.flush()

    write_record({
        'type': 'header',
        'format': 'see tests/fixtures/ftms/README.md; frame.hex is the raw '
                  '2ACD notification payload, little-endian FTMS, flags uint16 first',
        'captured_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'device': dev_alias,
        'address': dev_address,
        'supported_speed_range_2ad4': speed_range.hex() if speed_range else None,
        'supported_inclination_range_2ad5': incline_range.hex() if incline_range else None,
    })

    state = {'annotation': None, 'frames': 0}

    def on_notify(iface, changed_props, invalidated_props):
        value = changed_props.get('Value', None)
        if not value:
            return
        state['frames'] += 1
        write_record({
            'type': 'frame',
            't': time.monotonic(),
            'hex': bytes(value).hex(),
            'speed_annotation': state['annotation'],
        })

    tm_char.start_notify()
    tm_char.add_characteristic_cb(on_notify)

    loop_thread = threading.Thread(target=monitor.run, daemon=True)
    loop_thread.start()

    print()
    print(f"Capturing to {out_path}")
    print("Whenever the console display changes, type the shown speed (e.g. 3.5)")
    print("and press Enter. Enter 0 for a stopped belt. Type q to finish.")
    try:
        while True:
            line = input('> ').strip()
            if line.lower() in ('q', 'quit', 'exit'):
                break
            if not line:
                continue
            state['annotation'] = line
            write_record({'type': 'annotation', 't': time.monotonic(), 'speed': line})
            print(f"  annotated: {line} ({state['frames']} frames so far)")
    except (KeyboardInterrupt, EOFError):
        pass

    print(f"\nDone. {state['frames']} frames written to {out_path}")
    try:
        monitor.disconnect()
    except Exception:
        pass
    out.close()


if __name__ == '__main__':
    main()
