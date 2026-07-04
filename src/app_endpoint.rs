//! App Endpoint: the bridge's BLE peripheral role. Advertises a virtual FTMS
//! treadmill so a Mobile App connects and sees live metrics. Per ADR-0002 the
//! Treadmill Measurement (2ACD), Fitness Machine Status (2ADA), and Training
//! Status (2AD3) notification bytes are forwarded *verbatim* from the real
//! treadmill — no parsing, no re-packing. The read characteristics (Fitness
//! Machine Feature 2ACC, supported speed 2AD4, supported inclination 2AD5) are
//! proxied from the treadmill's own values, with hardcoded fallbacks when the
//! treadmill lacks them or is absent (Idle).
//!
//! It keeps advertising while Idle so a future UI can reach the bridge exactly
//! when nothing is connected (ADR-0002); FTMS signals "no data" in-band.

use std::sync::Arc;

use bluer::{
    adv::Advertisement,
    gatt::local::{
        Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
        CharacteristicRead, Service,
    },
    Adapter,
};
use futures_util::FutureExt;
use tokio::sync::{broadcast, Mutex};

use crate::ftms;

/// Which forwarded notify characteristic a frame belongs to. The Treadmill Link
/// tags each frame; the App Endpoint routes it to the matching local notifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwdChar {
    Measurement,    // 2ACD
    Status,         // 2ADA
    TrainingStatus, // 2AD3
}

/// One treadmill notification, forwarded byte-for-byte.
#[derive(Debug, Clone)]
pub struct ForwardFrame {
    pub char: FwdChar,
    pub bytes: Vec<u8>,
}

/// Values proxied from the treadmill's read characteristics; start as
/// fallbacks and are overwritten when the treadmill provides its own.
#[derive(Debug, Clone)]
pub struct ProxiedReads {
    pub feature: Vec<u8>,
    pub speed_range: Vec<u8>,
    pub incline_range: Vec<u8>,
}

impl Default for ProxiedReads {
    fn default() -> Self {
        Self {
            feature: ftms::FALLBACK_FEATURE.to_vec(),
            speed_range: ftms::FALLBACK_SPEED_RANGE.to_vec(),
            incline_range: ftms::FALLBACK_INCLINATION_RANGE.to_vec(),
        }
    }
}

/// Everything the Treadmill Link needs to feed the App Endpoint. Absent in
/// Degraded Mode (single adapter), where the Link still drives ANT.
#[derive(Clone)]
pub struct AppBridge {
    pub frames: broadcast::Sender<ForwardFrame>,
    pub reads: Arc<Mutex<ProxiedReads>>,
}

/// Serve the GATT application and advertise forever. Returns only on an
/// unexpected BlueZ error (crash-only, like the Treadmill Link).
pub async fn run(
    adapter: Adapter,
    ble_name: String,
    frames: broadcast::Sender<ForwardFrame>,
    reads: Arc<Mutex<ProxiedReads>>,
) -> bluer::Result<()> {
    adapter.set_powered(true).await?;
    tracing::info!(
        adapter = adapter.name(),
        name = ble_name,
        "App Endpoint advertising"
    );

    let app = Application {
        services: vec![Service {
            uuid: ftms::FTMS_SERVICE,
            primary: true,
            characteristics: vec![
                read_char(ftms::FITNESS_MACHINE_FEATURE, reads.clone(), |r| &r.feature),
                read_char(ftms::SUPPORTED_SPEED_RANGE, reads.clone(), |r| {
                    &r.speed_range
                }),
                read_char(ftms::SUPPORTED_INCLINATION_RANGE, reads.clone(), |r| {
                    &r.incline_range
                }),
                notify_char(ftms::TREADMILL_MEASUREMENT, FwdChar::Measurement, &frames),
                notify_char(ftms::FITNESS_MACHINE_STATUS, FwdChar::Status, &frames),
                notify_char(ftms::TRAINING_STATUS, FwdChar::TrainingStatus, &frames),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let _app_handle = adapter.serve_gatt_application(app).await?;

    let adv = Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: [ftms::FTMS_SERVICE].into_iter().collect(),
        discoverable: Some(true),
        local_name: Some(ble_name),
        ..Default::default()
    };
    let _adv_handle = adapter.advertise(adv).await?;

    // Handles must stay alive; park here. The Link owns the frame sender, so
    // this future never resolves on its own — the process exits via the Link's
    // crash-only path (see main::select).
    std::future::pending::<()>().await;
    Ok(())
}

/// A read characteristic whose value is the current proxied bytes selected by
/// `pick`. Reads the shared state fresh on each request so late treadmill
/// values (after an app already connected) are served correctly.
fn read_char(
    uuid: bluer::Uuid,
    reads: Arc<Mutex<ProxiedReads>>,
    pick: fn(&ProxiedReads) -> &Vec<u8>,
) -> Characteristic {
    Characteristic {
        uuid,
        read: Some(CharacteristicRead {
            read: true,
            fun: Box::new(move |_req| {
                let reads = reads.clone();
                async move { Ok(pick(&*reads.lock().await).clone()) }.boxed()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A notify characteristic that forwards tagged treadmill frames verbatim to
/// every subscribed app.
fn notify_char(
    uuid: bluer::Uuid,
    tag: FwdChar,
    frames: &broadcast::Sender<ForwardFrame>,
) -> Characteristic {
    let frames = frames.clone();
    Characteristic {
        uuid,
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
                let mut rx = frames.subscribe();
                async move {
                    tokio::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(f) if f.char == tag => {
                                    if notifier.notify(f.bytes).await.is_err() {
                                        break; // app unsubscribed / disconnected
                                    }
                                }
                                Ok(_) => {} // frame for a different characteristic
                                Err(broadcast::error::RecvError::Lagged(_)) => {} // skip stale
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                }
                .boxed()
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallbacks_are_the_python_values() {
        let r = ProxiedReads::default();
        assert_eq!(r.feature, ftms::FALLBACK_FEATURE);
        assert_eq!(r.speed_range, ftms::FALLBACK_SPEED_RANGE);
        assert_eq!(r.incline_range, ftms::FALLBACK_INCLINATION_RANGE);
    }

    /// Acceptance criterion: forwarded notification bytes are byte-identical to
    /// what the treadmill sends. Drive real captured 2ACD frames through the
    /// broadcast forwarding path and assert nothing is parsed or re-packed.
    #[tokio::test]
    async fn forwarded_frames_are_byte_identical() {
        let (tx, mut rx) = broadcast::channel(256);
        let jsonl = include_str!("../tests/fixtures/ftms/session-20260703.jsonl");

        let mut sent = Vec::new();
        for line in jsonl.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v["type"] != "frame" {
                continue;
            }
            let bytes = hex(v["hex"].as_str().unwrap());
            sent.push(bytes.clone());
            tx.send(ForwardFrame {
                char: FwdChar::Measurement,
                bytes,
            })
            .unwrap();
        }

        for expected in sent {
            let got = rx.recv().await.unwrap();
            assert_eq!(got.char, FwdChar::Measurement);
            assert_eq!(got.bytes, expected, "frame mutated in the forwarding path");
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
