//! Control Command routing: the write path from a Mobile App's FTMS Control
//! Point (2AD9) to the Treadmill's. Per ADR-0002 the Control Point path is
//! verbatim passthrough — the bridge parses nothing it forwards. The one thing
//! it *does* synthesize is a failure response when the treadmill is unreachable,
//! so an app's write reports a clean error instead of hanging for a response
//! that will never come.
//!
//! This is the pure seam for the write path (the mirror of `sdm` on the read
//! path): no I/O, just the forward-or-reject decision, so the acceptance
//! criteria are testable without a treadmill or a phone.

/// FTMS Control Point op-code marking a Response Code message (spec §4.16.2.22).
const RESPONSE_OP_CODE: u8 = 0x80;
/// FTMS result code 0x04 "Operation Failed": the machine could not service the
/// request. The honest answer when the real treadmill isn't connected.
const RESULT_OP_FAILED: u8 = 0x04;

/// What the bridge does with a Control Point write from an app.
#[derive(Debug, PartialEq, Eq)]
pub enum Routed {
    /// Forward these bytes verbatim to the treadmill's Control Point.
    Forward(Vec<u8>),
    /// Treadmill unreachable: deliver this synthesized FTMS Response Code
    /// indication back to the app so its write doesn't time out.
    Reject(Vec<u8>),
}

/// Route one Control Point write. Connected: forward verbatim (ADR-0002 — parse
/// nothing). Not connected: reject with an FTMS Response Code (Operation Failed)
/// echoing the requested op-code, rather than dropping the write silently.
pub fn route(bytes: &[u8], treadmill_connected: bool) -> Routed {
    if treadmill_connected {
        Routed::Forward(bytes.to_vec())
    } else {
        Routed::Reject(reject(bytes.first().copied().unwrap_or(0)))
    }
}

/// The FTMS Control Point "Operation Failed" Response Code indication for a
/// request the bridge can't deliver to the treadmill. Echoes the request
/// op-code per FTMS §4.16.2.22. Used both when no treadmill is connected and
/// when a connected treadmill exposes no Control Point to forward to.
pub fn reject(request_op: u8) -> Vec<u8> {
    vec![RESPONSE_OP_CODE, request_op, RESULT_OP_FAILED]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_forwards_verbatim() {
        // Set Target Speed (0x02) with a uint16 param: nothing is parsed or
        // altered on the way to the treadmill.
        let cmd = [0x02, 0x64, 0x00];
        assert_eq!(route(&cmd, true), Routed::Forward(cmd.to_vec()));
        // Request Control (0x00), the app's first write.
        assert_eq!(route(&[0x00], true), Routed::Forward(vec![0x00]));
    }

    #[test]
    fn disconnected_rejects_with_operation_failed() {
        // Response Code (0x80), echoed request op-code, Operation Failed (0x04).
        assert_eq!(
            route(&[0x02, 0x64, 0x00], false),
            Routed::Reject(vec![0x80, 0x02, 0x04])
        );
        assert_eq!(
            route(&[0x00], false),
            Routed::Reject(vec![0x80, 0x00, 0x04])
        );
    }

    #[test]
    fn disconnected_empty_write_still_rejects() {
        // A zero-length write can't name an op-code; echo 0x00, never panic.
        assert_eq!(route(&[], false), Routed::Reject(vec![0x80, 0x00, 0x04]));
    }

    #[test]
    fn reject_frame_shape() {
        // Same frame the Link sends when a connected treadmill has no Control
        // Point: Response Code (0x80), echoed op, Operation Failed (0x04).
        assert_eq!(reject(0x07), vec![0x80, 0x07, 0x04]);
    }
}
