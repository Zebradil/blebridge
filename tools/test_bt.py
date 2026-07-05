#!/usr/bin/env python3
"""Validate the Rust App Endpoint from a laptop BLE controller.

This is the automated equivalent of the nRF Connect smoke test: scan for the
bridge, connect as a BLE central, read the proxied FTMS range characteristics,
then subscribe to the treadmill notification characteristics long enough to see
whether live frames arrive.
"""

import argparse
import asyncio
from collections import Counter

from bleak import BleakClient, BleakScanner


CHARS = {
    "2ACD treadmill measurement": "00002acd-0000-1000-8000-00805f9b34fb",
    "2ADA fitness machine status": "00002ada-0000-1000-8000-00805f9b34fb",
    "2AD3 training status": "00002ad3-0000-1000-8000-00805f9b34fb",
}
READS = {
    "2AD4 supported speed range": "00002ad4-0000-1000-8000-00805f9b34fb",
    "2AD5 supported incline range": "00002ad5-0000-1000-8000-00805f9b34fb",
}


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--name",
        default="BLE_Bridge_Treadmill",
        help="advertised bridge name (default: BLE_Bridge_Treadmill)",
    )
    parser.add_argument(
        "--address",
        help="connect to this BLE address instead of scanning by name",
    )
    parser.add_argument(
        "--scan-timeout",
        type=float,
        default=20.0,
        help="seconds to scan for the bridge (default: 20)",
    )
    parser.add_argument(
        "--capture-seconds",
        type=float,
        default=30.0,
        help="seconds to listen for notifications (default: 30)",
    )
    parser.add_argument(
        "--allow-no-frames",
        action="store_true",
        help="exit successfully when no 2ACD frames arrive; useful for idle-mode checks",
    )
    return parser.parse_args()


async def find_bridge(args):
    if args.address:
        return args.address

    print(f"Scanning {args.scan_timeout:g}s for {args.name!r}...")

    # Match by name first. Matching only the FTMS UUID can accidentally pick the
    # physical treadmill when it is on, which proves the wrong thing for issue #6.
    device = await BleakScanner.find_device_by_filter(
        lambda d, ad: d.name == args.name or ad.local_name == args.name,
        timeout=args.scan_timeout,
    )
    if not device:
        raise SystemExit(
            f"Bridge {args.name!r} not found. Check App Endpoint logs and BLEBRIDGE_BLE_NAME."
        )
    return device


async def main():
    args = parse_args()
    target = await find_bridge(args)
    print(f"Connecting to {getattr(target, 'address', target)}...")

    async with BleakClient(target) as client:
        print("Connected")

        print("Reading proxied FTMS ranges:")
        for label, uuid in READS.items():
            try:
                value = await client.read_gatt_char(uuid)
                print(f"  {label}: {bytes(value).hex()}")
            except Exception as exc:
                print(f"  {label}: read failed: {type(exc).__name__}: {exc}")

        counts = Counter()

        def callback(label):
            def handle(_sender, data):
                counts[label] += 1
                # Raw hex is intentional: issue #6 requires byte-for-byte FTMS
                # passthrough, so this script displays bytes instead of parsing.
                print(f"{label}: {bytes(data).hex()}")

            return handle

        print("Subscribing to FTMS notifications:")
        for label, uuid in CHARS.items():
            try:
                await client.start_notify(uuid, callback(label))
                print(f"  {label}: subscribed")
            except Exception as exc:
                # Some treadmills may omit status/training chars. 2ACD is the
                # required live metric path for this smoke test.
                print(f"  {label}: subscribe failed: {type(exc).__name__}: {exc}")

        print(f"Listening {args.capture_seconds:g}s. Start walking if treadmill is on.")
        await asyncio.sleep(args.capture_seconds)

        print("Notification counts:")
        for label in CHARS:
            print(f"  {label}: {counts[label]}")

        if counts["2ACD treadmill measurement"] == 0 and not args.allow_no_frames:
            raise SystemExit("No 2ACD notifications seen; live App Endpoint path not validated.")


if __name__ == "__main__":
    asyncio.run(main())
