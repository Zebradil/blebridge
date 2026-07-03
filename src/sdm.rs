//! Pure SDM (Stride & Distance Sensor) page engine — the bridge core for the
//! ANT Broadcaster. No I/O, no clock reads: timestamps arrive injected via
//! [`Event::TxRequested`]. Math is a direct port of the Python
//! `AntSend.Create_Next_DataPage` so golden tests lock in existing behavior.

/// Fixed stride cadence in steps/min; SDM stride counting derives from it
/// and elapsed time (matches the Python implementation).
pub const CADENCE_SPM: f64 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    TreadmillConnected,
    TreadmillDisconnected,
    /// Instantaneous speed in m/s.
    SpeedUpdated(f64),
    /// A TX slot opened on the ANT channel; `timestamp` is seconds on any
    /// monotonic clock chosen by the adapter.
    TxRequested {
        timestamp: f64,
    },
}

#[derive(Debug, Default)]
pub struct SdmCore {
    connected: bool,
    speed: f64,
    message_count: u32,
    last_tx: Option<f64>,
    last_stride_time: f64,
    strides_done: u32,
    distance_accu: f64,
    distance_last: f64,
    speed_last: f64,
    time_rollover: f64,
}

impl SdmCore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one event. Returns page bytes to broadcast only for
    /// `TxRequested` while a Treadmill is connected; while Idle the
    /// broadcaster stays silent (silence is the diagnostic, per ADR-0002).
    pub fn handle(&mut self, event: Event) -> Option<[u8; 8]> {
        match event {
            Event::TreadmillConnected => {
                self.connected = true;
                None
            }
            Event::TreadmillDisconnected => {
                self.connected = false;
                None
            }
            Event::SpeedUpdated(mps) => {
                self.speed = mps;
                None
            }
            Event::TxRequested { timestamp } => {
                if !self.connected {
                    return None;
                }
                Some(self.next_page(timestamp))
            }
        }
    }

    fn next_page(&mut self, timestamp: f64) -> [u8; 8] {
        self.message_count += 1;

        let elapsed = match self.last_tx {
            Some(t) => (timestamp - t).clamp(0.0, 1.0),
            None => 0.0,
        };
        self.last_tx = Some(timestamp);
        let update_latency = ((elapsed / 0.03125) as u32).min(255) as u8;

        // One stride per two footfalls: at 160 steps/min that is one stride
        // every 0.75 s.
        let stride_period = 60.0 / (CADENCE_SPM / 2.0);
        while self.last_stride_time > stride_period {
            self.strides_done += 1;
            self.last_stride_time -= stride_period;
        }
        self.last_stride_time += elapsed;
        self.strides_done %= 256;

        // Accumulated distance in meters, rollover at 256.
        self.distance_accu = (self.distance_accu + elapsed * self.speed) % 256.0;
        let distance_int = self.distance_accu as u8;
        let distance_frac16 = ((self.distance_accu - distance_int as f64) * 16.0) as u8;

        let speed_int = self.speed as u8;
        let speed_frac256 = ((self.speed - speed_int as f64) * 256.0) as u8;

        // Per SDM spec, accumulated time only advances while speed or
        // distance change; rollover at 256 s.
        if self.speed_last != self.speed || self.distance_last != self.distance_accu {
            self.time_rollover = (self.time_rollover + elapsed) % 256.0;
        }
        let time_int = self.time_rollover as u8;
        let time_frac200 = ((self.time_rollover - time_int as f64) * 200.0) as u8;
        self.speed_last = self.speed;
        self.distance_last = self.distance_accu;

        // Page rotation ported verbatim: 80 for messages 1-2, 81 for 65-66,
        // page 1 otherwise, cycle length 132.
        if self.message_count < 3 {
            [80, 0xFF, 0xFF, 1, 1, 1, 1, 1]
        } else if self.message_count > 64 && self.message_count < 67 {
            [81, 0xFF, 0xFF, 1, 0xFF, 0xFF, 0xFF, 0xFF]
        } else {
            if self.message_count > 131 {
                self.message_count = 0;
            }
            [
                1,
                time_frac200,
                time_int,
                distance_int,
                (distance_frac16 << 4) | (speed_int & 0x0F),
                speed_frac256,
                self.strides_done as u8,
                update_latency,
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected_core(speed: f64) -> SdmCore {
        let mut core = SdmCore::new();
        assert_eq!(core.handle(Event::TreadmillConnected), None);
        assert_eq!(core.handle(Event::SpeedUpdated(speed)), None);
        core
    }

    #[test]
    fn silent_unless_connected() {
        let mut core = SdmCore::new();
        assert_eq!(core.handle(Event::TxRequested { timestamp: 0.0 }), None);

        core.handle(Event::TreadmillConnected);
        assert!(core.handle(Event::TxRequested { timestamp: 1.0 }).is_some());

        core.handle(Event::TreadmillDisconnected);
        assert_eq!(core.handle(Event::TxRequested { timestamp: 2.0 }), None);
    }

    #[test]
    fn page_rotation_schedule() {
        let mut core = connected_core(1.0);
        let pages: Vec<u8> = (0..134)
            .map(|k| {
                core.handle(Event::TxRequested {
                    timestamp: k as f64 * 0.25,
                })
                .unwrap()[0]
            })
            .collect();

        // 1-based message positions: 80 for 1-2, 81 for 65-66, 1 elsewhere;
        // counter resets after 132 so 133-134 are page 80 again.
        for (i, page) in pages.iter().enumerate() {
            let n = i + 1;
            let expected = match n {
                1 | 2 | 133 | 134 => 80,
                65 | 66 => 81,
                _ => 1,
            };
            assert_eq!(*page, expected, "message {n}");
        }
    }

    #[test]
    fn golden_page1_constant_speed() {
        // 1.5 m/s, TX every 250 ms. Third message is the first page 1:
        // elapsed 0.25 s, distance 0.75 m, time 0.5 s, no strides yet,
        // update latency 0.25/0.03125 = 8.
        let mut core = connected_core(1.5);
        core.handle(Event::TxRequested { timestamp: 0.0 });
        core.handle(Event::TxRequested { timestamp: 0.25 });
        let page = core.handle(Event::TxRequested { timestamp: 0.5 }).unwrap();
        assert_eq!(page, [1, 100, 0, 0, 0xC1, 128, 0, 8]);
    }

    #[test]
    fn golden_stride_distance_and_time_rollover() {
        // 4.0 m/s, TX every 750 ms (exactly one stride period at the fixed
        // 160 steps/min cadence). At message 87: distance 3*86 = 258 m has
        // rolled over to 2, time 0.75*86 = 64.5 s, strides 84, update
        // latency 0.75/0.03125 = 24.
        let mut core = connected_core(4.0);
        let mut last = None;
        for k in 0..87 {
            last = core.handle(Event::TxRequested {
                timestamp: k as f64 * 0.75,
            });
        }
        assert_eq!(last.unwrap(), [1, 100, 64, 2, 4, 0, 84, 24]);
    }
}
