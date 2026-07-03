//! Treadmill Link: the bridge's BLE central role. Keeps a continuous discovery
//! session open (no scan backoff, per ADR-0001), connects to the Treadmill's
//! FTMS peripheral within seconds of it powering on, subscribes to Treadmill
//! Measurement notifications, and drives the shared [`SdmCore`] with speed
//! updates. On disconnect it returns to Idle and rediscovers automatically.
//!
//! Crash-only: transient "treadmill went away" is normal and re-loops; any
//! unexpected BlueZ/D-Bus error propagates out so the process exits nonzero and
//! Docker restarts it.

use std::sync::{Arc, Mutex};

use bluer::{Adapter, AdapterEvent, Address, Device, Uuid};
use futures_util::StreamExt;

use crate::sdm::{Event, SdmCore};

// FTMS (Fitness Machine Service) and its Treadmill Measurement characteristic,
// as 128-bit forms of the SIG 16-bit UUIDs 0x1826 / 0x2ACD.
const FTMS_SERVICE: Uuid = Uuid::from_u128(0x00001826_0000_1000_8000_00805f9b34fb);
const TREADMILL_MEASUREMENT: Uuid = Uuid::from_u128(0x00002acd_0000_1000_8000_00805f9b34fb);

// The bridge's own App Endpoint advertises under this alias; never treat it as
// the Treadmill even though it exposes FTMS.
const OWN_PERIPHERAL_ALIAS: &str = "BLE_Bridge_Treadmill";

/// Run the Treadmill Link forever. Returns only on an unexpected error (crash-only).
pub async fn run(core: Arc<Mutex<SdmCore>>, pin: Option<Address>) -> bluer::Result<()> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    tracing::info!(
        adapter = adapter.name(),
        pinned = ?pin,
        "Treadmill Link discovery starting"
    );

    loop {
        let device = discover(&adapter, pin).await?;
        // A connect/subscribe failure or mid-session disconnect returns to Idle
        // and rediscovers; it is not a crash.
        if let Err(e) = serve(&core, &device).await {
            tracing::warn!(addr = %device.address(), "treadmill session ended: {e}");
        }
        core.lock().unwrap().handle(Event::TreadmillDisconnected);
        tracing::info!("back to Idle; rediscovering");
    }
}

/// Continuous scan until a Treadmill appears. No backoff — the discovery stream
/// stays open and yields the device as soon as BlueZ sees it.
async fn discover(adapter: &Adapter, pin: Option<Address>) -> bluer::Result<Device> {
    let mut events = std::pin::pin!(adapter.discover_devices().await?);
    // discover_devices replays already-known devices as DeviceAdded, so a
    // treadmill still cached from a prior session is picked up immediately.
    while let Some(event) = events.next().await {
        let AdapterEvent::DeviceAdded(addr) = event else {
            continue;
        };
        let device = adapter.device(addr)?;
        if is_treadmill(&device, pin).await? {
            tracing::info!(%addr, "treadmill found");
            return Ok(device);
        }
    }
    // The stream is infinite in practice; if BlueZ ever closes it, that is
    // unexpected — surface it as a crash-only error.
    Err(bluer::Error {
        kind: bluer::ErrorKind::Failed,
        message: "discovery stream ended unexpectedly".into(),
    })
}

async fn is_treadmill(device: &Device, pin: Option<Address>) -> bluer::Result<bool> {
    if let Some(mac) = pin {
        // Pinned: the MAC is the whole match; ignore everything else.
        return Ok(device.address() == mac);
    }
    // Unpinned: first FTMS advertiser wins, except our own App Endpoint.
    // ponytail: relies on the FTMS 0x1826 UUID being in the advertisement at
    // DeviceAdded time (standard for FTMS treadmills). If a device turns out to
    // report UUIDs only after a PropertyChanged, pin its MAC via
    // BLEBRIDGE_TREADMILL_MAC instead of teaching discover() to re-check.
    let advertises_ftms = device
        .uuids()
        .await?
        .is_some_and(|uuids| uuids.contains(&FTMS_SERVICE));
    if !advertises_ftms {
        return Ok(false);
    }
    let is_own = device.alias().await? == OWN_PERIPHERAL_ALIAS;
    Ok(!is_own)
}

/// Connect, subscribe to Treadmill Measurement, and pump speed updates into the
/// core until the notification stream ends (disconnect).
async fn serve(core: &Arc<Mutex<SdmCore>>, device: &Device) -> bluer::Result<()> {
    if !device.is_connected().await? {
        device.connect().await?;
    }
    let measurement = find_measurement(device).await?;
    let mut frames = std::pin::pin!(measurement.notify().await?);
    core.lock().unwrap().handle(Event::TreadmillConnected);
    tracing::info!(addr = %device.address(), "treadmill connected, streaming speed");

    while let Some(frame) = frames.next().await {
        if let Some(mps) = crate::ftms::instantaneous_speed_mps(&frame) {
            core.lock().unwrap().handle(Event::SpeedUpdated(mps));
        }
    }
    Ok(())
}

async fn find_measurement(device: &Device) -> bluer::Result<bluer::gatt::remote::Characteristic> {
    for service in device.services().await? {
        if service.uuid().await? != FTMS_SERVICE {
            continue;
        }
        for ch in service.characteristics().await? {
            if ch.uuid().await? == TREADMILL_MEASUREMENT {
                return Ok(ch);
            }
        }
    }
    Err(bluer::Error {
        kind: bluer::ErrorKind::NotFound,
        message: "Treadmill Measurement characteristic (2ACD) not found".into(),
    })
}
