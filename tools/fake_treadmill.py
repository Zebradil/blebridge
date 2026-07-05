#!/usr/bin/env python3
"""Advertise a fake FTMS treadmill from the laptop's BLE controller.

Lets the bridge's Treadmill Link connect to something real without the
physical treadmill: replays captured 2ACD frames from the fixtures at ~1.6 Hz.
Combined with tools/test_bt.py (laptop as app client) this gives a full
laptop-only loopback test of the bridge:

    uv run --no-project --with bluez-peripheral python tools/fake_treadmill.py &
    uv run --no-project --with bleak python tools/test_bt.py

Requires BlueZ on the laptop. Ctrl-C to stop.
"""

import argparse
import asyncio
import json
import pathlib

from bluez_peripheral.advert import Advertisement
from bluez_peripheral.agent import NoIoAgent
from bluez_peripheral.gatt.characteristic import CharacteristicFlags as CharFlags
from bluez_peripheral.gatt.characteristic import characteristic
from bluez_peripheral.gatt.service import Service
from bluez_peripheral.util import Adapter, get_message_bus

FIXTURE = pathlib.Path(__file__).parent.parent / "tests/fixtures/ftms/session-20260703.jsonl"


def load_fixture():
    """Return (speed_range_bytes, list of 2ACD frame bytes)."""
    speed_range = b"\x64\x00\x20\x03\x0a\x00"
    frames = []
    for line in FIXTURE.read_text().splitlines():
        v = json.loads(line)
        if v["type"] == "header" and v.get("supported_speed_range_2ad4"):
            speed_range = bytes.fromhex(v["supported_speed_range_2ad4"])
        elif v["type"] == "frame":
            # All fixture frames are raw 2ACD payloads (full 19-byte and short
            # 5-byte variants alternate, matching the real treadmill cadence).
            frames.append(bytes.fromhex(v["hex"]))
    return speed_range, frames


class FakeFtms(Service):
    def __init__(self, speed_range):
        super().__init__("1826", True)
        self._speed_range = speed_range

    # Fitness Machine Feature: avg speed + total distance (matches fixtures).
    @characteristic("2ACC", CharFlags.READ)
    def feature(self, options):
        return bytes.fromhex("0300000000000000")

    @characteristic("2AD4", CharFlags.READ)
    def speed_range(self, options):
        return self._speed_range

    @characteristic("2ACD", CharFlags.NOTIFY)
    def measurement(self, options):
        return b""  # value only pushed via changed()

    def push(self, frame: bytes):
        self.measurement.changed(frame)


async def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--name", default="FakeTreadmill")
    parser.add_argument("--rate", type=float, default=1.6, help="frames per second")
    args = parser.parse_args()

    speed_range, frames = load_fixture()
    print(f"Loaded {len(frames)} 2ACD frames from {FIXTURE.name}")

    bus = await get_message_bus()
    service = FakeFtms(speed_range)
    await service.register(bus)
    await NoIoAgent().register(bus)

    adapter = await Adapter.get_first(bus)
    advert = Advertisement(args.name, ["1826"], 0x0000, timeout=0)
    await advert.register(bus, adapter)
    print(f"Advertising {args.name!r} with FTMS 1826. Ctrl-C to stop.")

    i = 0
    while True:
        service.push(frames[i % len(frames)])
        i += 1
        if i % 50 == 0:
            print(f"pushed {i} frames")
        await asyncio.sleep(1.0 / args.rate)


if __name__ == "__main__":
    asyncio.run(main())
