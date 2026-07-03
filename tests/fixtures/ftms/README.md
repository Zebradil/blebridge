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

## Capturing a session

On the Pi (bluezero installed, treadmill powered on):

```bash
python3 tools/capture_ftms.py --out tests/fixtures/ftms/session-$(date +%Y%m%d).jsonl
```

Cover at least: belt stopped, two distinct speeds, and a speed transition.
Type the console speed + Enter whenever the display changes; `q` to finish.
