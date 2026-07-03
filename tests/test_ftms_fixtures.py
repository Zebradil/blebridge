"""Self-check for the FTMS fixture format (tests/fixtures/ftms/README.md).

Reference parser: raw 2ACD frame -> (flags, instantaneous speed in km/h).
Runnable with pytest or plain `python3 tests/test_ftms_fixtures.py`.
"""

import struct


def parse_frame(hex_frame):
    """Parse a raw 2ACD Treadmill Measurement frame (hex string).

    Returns (flags, speed_kmh). flags is the uint16 little-endian word at
    offset 0. speed_kmh is the instantaneous speed (uint16 LE at offset 2,
    0.01 km/h resolution) when flags bit 0 is 0, else None (bit 0 set =
    "More Data" = speed field absent, per FTMS spec).
    """
    raw = bytes.fromhex(hex_frame)
    flags = struct.unpack_from('<H', raw, 0)[0]
    if flags & 0x0001:
        return flags, None
    return flags, struct.unpack_from('<H', raw, 2)[0] / 100.0


def test_stopped_belt():
    # flags=0x0000 (speed present), speed=0
    assert parse_frame('00000000') == (0x0000, 0.0)


def test_walking_speed():
    # flags=0x0000, speed=350 (0x015E LE) -> 3.50 km/h
    assert parse_frame('00005e01') == (0x0000, 3.5)


def test_speed_with_distance_and_time():
    # flags=0x0404 (bit2 total distance, bit10 elapsed time), speed=520
    # (0x0208 LE -> 5.20 km/h), distance=100 m (uint24 LE), time=60 s
    flags, speed = parse_frame('04040802' + '640000' + '3c00')
    assert flags == 0x0404
    assert speed == 5.2
    # trailing fields decodable per the documented table
    raw = bytes.fromhex('04040802640000' + '3c00')
    assert raw[4] | raw[5] << 8 | raw[6] << 16 == 100
    assert struct.unpack_from('<H', raw, 7)[0] == 60


def test_more_data_frame_has_no_speed():
    # flags bit 0 set -> speed field absent
    assert parse_frame('0100') == (0x0001, None)


if __name__ == '__main__':
    for name, fn in sorted(globals().items()):
        if name.startswith('test_'):
            fn()
            print(f"{name}: ok")
    print("all checks passed")
