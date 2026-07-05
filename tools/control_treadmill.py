#!/usr/bin/env python3
"""Drive the bridge's FTMS Control Point (2AD9) from the laptop.

Purpose: test app-style control without a phone, and see the treadmill's own
Control Point responses (which explain why an app like FitShow refuses control).

Uses ATT Write WITHOUT Response (the path that works around the BlueZ 5.55
write-with-response bug). Subscribes to the Control Point indication, Fitness
Machine Status (2ADA) and Training Status (2AD3) and prints every frame decoded,
so you can see whether the treadmill GRANTS control.

  ┌───────────────────────────────────────────────────────────────────────┐
  │ SAFETY: --start and --speed MOVE THE BELT. Be on the treadmill or clear │
  │ of it, one hand on the rail. Ctrl-C or --stop halts. Default run only   │
  │ requests control and observes — it does not move the belt.             │
  └───────────────────────────────────────────────────────────────────────┘

Run (needs bleak; no repo env required):
    uv run --no-project --with bleak python tools/control_treadmill.py            # observe only
    uv run --no-project --with bleak python tools/control_treadmill.py --start --speed 3.0
    uv run --no-project --with bleak python tools/control_treadmill.py --stop
"""
import argparse
import asyncio
import struct

from bleak import BleakClient, BleakScanner

NAME = "BLE_Bridge_Treadmill"
CP = "00002ad9-0000-1000-8000-00805f9b34fb"        # Fitness Machine Control Point
STATUS = "00002ada-0000-1000-8000-00805f9b34fb"    # Fitness Machine Status
TRAINING = "00002ad3-0000-1000-8000-00805f9b34fb"  # Training Status

# FTMS Control Point op-codes we may send.
REQUEST_CONTROL = 0x00
RESET = 0x01
SET_TARGET_SPEED = 0x02
START_RESUME = 0x07
STOP_PAUSE = 0x08

RESULT = {
    0x01: "Success",
    0x02: "Op Code Not Supported",
    0x03: "Invalid Parameter",
    0x04: "Operation Failed",
    0x05: "Control Not Permitted",  # <-- treadmill refusing control lands here
}
OPNAME = {0x00: "RequestControl", 0x01: "Reset", 0x02: "SetTargetSpeed", 0x07: "Start", 0x08: "Stop"}


def decode_cp(data: bytes) -> str:
    if len(data) >= 3 and data[0] == 0x80:
        op, res = data[1], data[2]
        return f"Response to {OPNAME.get(op, hex(op))} -> {RESULT.get(res, f'0x{res:02x}')}"
    return f"raw {data.hex()}"


async def send(client, name, payload: bytes):
    print(f"--> {name}: {payload.hex()}", flush=True)
    # response=False == ATT Write Command; avoids the BlueZ 5.55 0x0E bug.
    await client.write_gatt_char(CP, payload, response=False)


async def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--address", help="connect by address instead of scanning by name")
    p.add_argument("--adapter", help="HCI adapter to use, e.g. hci2 (default: system default)")
    p.add_argument("--start", action="store_true", help="send Start/Resume (MOVES THE BELT)")
    p.add_argument("--speed", type=float, help="set target speed in km/h (MOVES THE BELT)")
    p.add_argument("--stop", action="store_true", help="send Stop/Pause")
    p.add_argument("--observe", type=float, default=8.0, help="seconds to watch responses (default 8)")
    args = p.parse_args()

    kw = {"adapter": args.adapter} if args.adapter else {}
    if args.address:
        print(f"scanning for {args.address} ...", flush=True)
        target = await BleakScanner.find_device_by_address(args.address, timeout=30, **kw)
        if not target:
            raise SystemExit(f"{args.address} not found — is it powered and advertising?")
    else:
        print(f"scanning for {NAME} ...", flush=True)
        target = await BleakScanner.find_device_by_name(NAME, timeout=20, **kw)
        if not target:
            raise SystemExit("bridge not found — is the App Endpoint advertising?")

    async with BleakClient(target) as c:
        print("connected", flush=True)

        def on_cp(_h, d):
            print(f"<== 2AD9 CP  : {decode_cp(bytes(d))}", flush=True)

        def on_status(_h, d):
            print(f"<== 2ADA stat: {bytes(d).hex()}", flush=True)

        def on_training(_h, d):
            print(f"<== 2AD3 trn : {bytes(d).hex()}", flush=True)

        await c.start_notify(CP, on_cp)
        for uuid, cb in ((STATUS, on_status), (TRAINING, on_training)):
            try:
                await c.start_notify(uuid, cb)
            except Exception as e:
                print(f"(subscribe {uuid[4:8]} failed: {e})", flush=True)

        # Always request control first; watch for Success vs Control Not Permitted.
        await send(c, "RequestControl(0x00)", bytes([REQUEST_CONTROL]))
        await asyncio.sleep(2)

        if args.start:
            await send(c, "Start(0x07)", bytes([START_RESUME]))
            await asyncio.sleep(2)
        if args.speed is not None:
            hundredths = round(args.speed * 100)
            await send(c, f"SetTargetSpeed({args.speed} km/h)",
                       bytes([SET_TARGET_SPEED]) + struct.pack("<H", hundredths))
            await asyncio.sleep(2)
        if args.stop:
            await send(c, "Stop(0x08)", bytes([STOP_PAUSE, 0x01]))
            await asyncio.sleep(2)

        print(f"observing {args.observe:g}s ...", flush=True)
        await asyncio.sleep(args.observe)
    print("done", flush=True)


asyncio.run(main())
