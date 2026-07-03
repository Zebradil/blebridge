# FTMS treadmill fixtures

Raw FTMS frames captured from the real treadmill with `tools/capture_ftms.py`.
These fixtures lock the Rust speed extraction and range proxying to the real
device's behavior (issue #2). Tests must be able to consume them without the
Python app — everything needed to parse is documented here.

## File format

One capture session = one `*.jsonl` file: one JSON object per line.

### `header` record (first line)

```json
{"type": "header",
 "captured_at": "<ISO 8601 UTC>",
 "device": "<BLE alias>", "address": "<MAC>",
 "supported_speed_range_2ad4": "<hex>" | null,
 "supported_inclination_range_2ad5": "<hex>" | null}
```

`null` means the treadmill does not expose that characteristic (or the read
failed) — proxying code needs a fallback in that case.

### `annotation` record

```json
{"type": "annotation", "t": <float>, "speed": "<console display, e.g. 3.5>"}
```

Written when the operator typed what the treadmill console showed. Speed is
in km/h as displayed; `0` means belt stopped.

### `frame` record

```json
{"type": "frame", "t": <float>, "hex": "<hex>", "speed_annotation": "3.5" | null}
```

- `t` — `time.monotonic()` seconds; only differences within one file are
  meaningful.
- `hex` — the **raw 2ACD (Treadmill Measurement) notification payload**,
  hex-encoded, exactly as received over GATT. Nothing stripped or reordered.
- `speed_annotation` — the last `annotation` speed at capture time (`null`
  before the first annotation).

## Parsing a 2ACD frame

All multi-byte fields are **little-endian** (FTMS spec, Bluetooth SIG).
The frame starts with a **flags word: uint16 at offset 0**. Fields follow in
bit order; each is present only if its flag says so:

| Bit | Meaning (1 = present unless noted)      | Field size / unit               |
|-----|------------------------------------------|---------------------------------|
| 0   | More Data — **0 means Instantaneous Speed IS present** (inverted!) | uint16, 0.01 km/h |
| 1   | Average Speed                             | uint16, 0.01 km/h               |
| 2   | Total Distance                            | uint24 (3 bytes), metres        |
| 3   | Inclination + Ramp Angle                  | sint16 (0.1 %) + sint16 (0.1°)  |
| 4   | Positive + Negative Elevation Gain        | uint16 + uint16, 0.1 m          |
| 5   | Instantaneous Pace                        | uint8, 0.1 km/min               |
| 6   | Average Pace                              | uint8, 0.1 km/min               |
| 7   | Expended Energy (total, per hr, per min)  | uint16 + uint16 + uint8, kcal   |
| 8   | Heart Rate                                | uint8, bpm                      |
| 9   | Metabolic Equivalent                      | uint8, 0.1 MET                  |
| 10  | Elapsed Time                              | uint16, s                       |
| 11  | Remaining Time                            | uint16, s                       |

So when flags bit 0 is 0, **Instantaneous Speed is the uint16 at offset 2,
in units of 0.01 km/h** — the field the ANT Broadcaster needs.

A minimal reference parser (flags + speed) with hand-built example frames
lives in `tests/test_ftms_fixtures.py`; run it to validate this format.

## Range characteristics (header record)

- `supported_speed_range_2ad4` — 6 bytes: min speed (uint16), max speed
  (uint16), min increment (uint16), all 0.01 km/h.
- `supported_inclination_range_2ad5` — 6 bytes: min (sint16), max (sint16),
  increment (uint16), all 0.1 %.

**The declared range can be wrong.** `session-20260703.jsonl` reports
`2ad4` max = 8.00 km/h, but the physical belt goes to 12.00 km/h. Speed
extraction must NOT clamp to `2ad4`, and range-proxy code cannot trust it as
the true maximum — verify against `2acd` Instantaneous Speed samples instead.

## Capturing a session

The Pi host has no `bluezero`; run the capture inside the deployed
`blebridge` image (it has the deps + D-Bus access). One physical treadmill
allows one BLE central, so stop the running bridge first, then restart it.

On the Pi:

```bash
# copy the tool onto the Pi once (from a repo checkout)
scp tools/capture_ftms.py suok@<pi>:~/capture_ftms.py

# stop bridge (frees treadmill) -> capture -> always restart bridge
docker stop blebridge && \
docker run --rm -it --network host --cap-add NET_ADMIN \
  -v /var/run/dbus:/var/run/dbus \
  -v ~/capture_ftms.py:/capture_ftms.py \
  -v ~/ftms:/out \
  blebridge:latest \
  python3 /capture_ftms.py --adapter 00:1A:7D:DA:71:0B \
    --out /out/session-$(date +%Y%m%d).jsonl ; \
docker start blebridge

# pull the fixture back into the repo
scp suok@<pi>:~/ftms/session-*.jsonl tests/fixtures/ftms/
```

`--adapter` is the USB dongle MAC; omit to use the first adapter. Cover at
least: belt stopped, two distinct speeds, and a speed transition (ramp).
Type the console speed + Enter whenever the display changes; `q` to finish.
