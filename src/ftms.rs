//! FTMS Treadmill Measurement (2ACD) parsing. Per ADR-0002 the treadmill path
//! is passthrough — only the ANT Broadcaster parses, and only the one field it
//! needs: instantaneous speed.

use bluer::Uuid;

// FTMS service and its characteristics, as 128-bit forms of the SIG 16-bit UUIDs.
pub const FTMS_SERVICE: Uuid = Uuid::from_u128(0x00001826_0000_1000_8000_00805f9b34fb);
pub const FITNESS_MACHINE_FEATURE: Uuid = Uuid::from_u128(0x00002acc_0000_1000_8000_00805f9b34fb);
pub const TREADMILL_MEASUREMENT: Uuid = Uuid::from_u128(0x00002acd_0000_1000_8000_00805f9b34fb);
pub const TRAINING_STATUS: Uuid = Uuid::from_u128(0x00002ad3_0000_1000_8000_00805f9b34fb);
pub const SUPPORTED_SPEED_RANGE: Uuid = Uuid::from_u128(0x00002ad4_0000_1000_8000_00805f9b34fb);
pub const SUPPORTED_INCLINATION_RANGE: Uuid =
    Uuid::from_u128(0x00002ad5_0000_1000_8000_00805f9b34fb);
pub const FITNESS_MACHINE_CONTROL_POINT: Uuid =
    Uuid::from_u128(0x00002ad9_0000_1000_8000_00805f9b34fb);
pub const FITNESS_MACHINE_STATUS: Uuid = Uuid::from_u128(0x00002ada_0000_1000_8000_00805f9b34fb);

// Hardcoded fallbacks served by the App Endpoint when the treadmill lacks the
// matching read characteristic (values ported from the Python peripheral).
pub const FALLBACK_FEATURE: [u8; 8] = [0x0D, 0x16, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00];
pub const FALLBACK_SPEED_RANGE: [u8; 6] = [0x64, 0x00, 0x40, 0x06, 0x0A, 0x00]; // 1.00–16.00 km/h, 0.10 step
pub const FALLBACK_INCLINATION_RANGE: [u8; 6] = [0x00, 0x00, 0x64, 0x00, 0x05, 0x00]; // 0–10.0%, 0.05 step

/// Instantaneous speed in m/s from a raw 2ACD notification payload, or `None`
/// when the frame carries no speed field.
///
/// The frame opens with a uint16 little-endian flags word (FTMS spec). Bit 0 is
/// "More Data": when *clear* the Instantaneous Speed field is present as a
/// uint16 at offset 2 in units of 0.01 km/h; when *set* the field is absent.
pub fn instantaneous_speed_mps(frame: &[u8]) -> Option<f64> {
    if frame.len() < 4 {
        return None;
    }
    let flags = u16::from_le_bytes([frame[0], frame[1]]);
    if flags & 0x0001 != 0 {
        return None; // More Data: speed field absent
    }
    let hundredths_kmh = u16::from_le_bytes([frame[2], frame[3]]);
    // 0.01 km/h -> m/s: /100 to km/h, /3.6 to m/s == /360.
    Some(hundredths_kmh as f64 / 360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kmh(frame: &[u8]) -> Option<f64> {
        instantaneous_speed_mps(frame).map(|mps| mps * 3.6)
    }

    #[test]
    fn hand_built_frames() {
        // flags=0x0000 (speed present), speed field 0 -> 0 km/h.
        assert_eq!(kmh(&hex("00000000")), Some(0.0));
        // speed=350 (0x015E LE) -> 3.50 km/h.
        assert_eq!(kmh(&hex("00005e01")), Some(3.5));
        // flags bit0 set -> More Data -> no speed.
        assert_eq!(instantaneous_speed_mps(&hex("0100")), None);
        // too short.
        assert_eq!(instantaneous_speed_mps(&hex("00")), None);
    }

    /// Every distinct instantaneous speed the extractor sees across a captured
    /// session, with occurrence counts. Locks extraction to the real device's
    /// frames (issue #5 acceptance criterion). More-Data frames yield no speed
    /// and are excluded.
    fn speed_histogram(jsonl: &str) -> std::collections::BTreeMap<String, usize> {
        let mut hist = std::collections::BTreeMap::new();
        for line in jsonl.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v["type"] != "frame" {
                continue;
            }
            let frame = hex(v["hex"].as_str().unwrap());
            if let Some(mps) = instantaneous_speed_mps(&frame) {
                // key by 0.01 km/h so floats compare exactly.
                let key = format!("{:.2}", mps * 3.6);
                *hist.entry(key).or_default() += 1;
            }
        }
        hist
    }

    #[test]
    fn fixture_walking_session() {
        let hist = speed_histogram(include_str!(
            "../tests/fixtures/ftms/session-20260703.jsonl"
        ));
        let expect = [
            ("0.00", 63),
            ("1.00", 15),
            ("3.00", 30),
            ("3.50", 6),
            ("4.00", 12),
        ];
        assert_eq!(hist.len(), expect.len(), "unexpected speeds: {hist:?}");
        for (k, n) in expect {
            assert_eq!(hist.get(k), Some(&n), "speed {k} km/h count");
        }
    }

    #[test]
    fn fixture_running_session() {
        let hist = speed_histogram(include_str!(
            "../tests/fixtures/ftms/session-20260703-highspeed.jsonl"
        ));
        // Running mode reaches 12.0 km/h — above the walking-mode 2ad4 max of
        // 8.0 — proving extraction never clamps to the advertised range.
        let expect = [
            ("0.00", 37),
            ("1.50", 1),
            ("2.50", 12),
            ("3.00", 26),
            ("3.50", 1),
            ("6.00", 1),
            ("8.00", 1),
            ("10.50", 1),
            ("12.00", 33),
        ];
        assert_eq!(hist.len(), expect.len(), "unexpected speeds: {hist:?}");
        for (k, n) in expect {
            assert_eq!(hist.get(k), Some(&n), "speed {k} km/h count");
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
