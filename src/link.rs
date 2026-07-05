//! Treadmill Link: the bridge's BLE central role. Keeps a continuous discovery
//! session open (no scan backoff, per ADR-0001), connects to the Treadmill's
//! FTMS peripheral within seconds of it powering on, subscribes to Treadmill
//! Measurement notifications, and drives the shared [`SdmCore`] with speed
//! updates. When an [`AppBridge`] is present it also subscribes to Fitness
//! Machine Status and Training Status and forwards all three verbatim to the App
//! Endpoint, and proxies the treadmill's read characteristics. On disconnect it
//! returns to Idle and rediscovers automatically.
//!
//! Crash-only: transient "treadmill went away" is normal and re-loops; any
//! unexpected BlueZ/D-Bus error propagates out so the process exits nonzero and
//! Docker restarts it.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use bluer::{Adapter, AdapterEvent, Address, Device, Uuid};
use futures_util::{stream::select_all, StreamExt};

use crate::app_endpoint::{AppBridge, ForwardFrame, FwdChar, ProxiedReads};
use crate::command;
use crate::ftms;
use crate::sdm::{Event, SdmCore};

/// Run the Treadmill Link forever. Returns only on an unexpected error (crash-only).
pub async fn run(
    adapter: Adapter,
    core: Arc<Mutex<SdmCore>>,
    pin: Option<Address>,
    bridge: Option<AppBridge>,
    own_name: String,
) -> bluer::Result<()> {
    adapter.set_powered(true).await?;
    tracing::info!(
        adapter = adapter.name(),
        pinned = ?pin,
        forwarding = bridge.is_some(),
        "Treadmill Link discovery starting"
    );

    loop {
        let device = discover(&adapter, pin, &own_name).await?;
        // A connect/subscribe failure or mid-session disconnect returns to Idle
        // and rediscovers; it is not a crash.
        if let Err(e) = serve(&core, &device, bridge.as_ref()).await {
            tracing::warn!(addr = %device.address(), "treadmill session ended: {e}");
        }
        core.lock().unwrap().handle(Event::TreadmillDisconnected);
        // Back to Idle: reject Control Commands again until a treadmill reconnects.
        if let Some(bridge) = &bridge {
            bridge.connected.store(false, Ordering::Relaxed);
        }
        tracing::info!("back to Idle; rediscovering");
    }
}

/// Continuous scan until a Treadmill appears. No backoff — the discovery stream
/// stays open and yields the device as soon as BlueZ sees it.
async fn discover(
    adapter: &Adapter,
    pin: Option<Address>,
    own_name: &str,
) -> bluer::Result<Device> {
    // _with_changes, not plain discover_devices: BlueZ fills device properties
    // (incl. the advertised FTMS UUID) asynchronously and often AFTER the first
    // DeviceAdded, so a plain stream checks is_treadmill once against empty UUIDs
    // and never again. The _with_changes variant re-emits DeviceAdded whenever
    // properties change, so the late-arriving UUID triggers a fresh check.
    // Already-known devices are replayed too, so a cached treadmill is immediate.
    let mut events = std::pin::pin!(adapter.discover_devices_with_changes().await?);
    while let Some(event) = events.next().await {
        let AdapterEvent::DeviceAdded(addr) = event else {
            continue;
        };
        let device = adapter.device(addr)?;
        if is_treadmill(&device, pin, own_name).await? {
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

async fn is_treadmill(
    device: &Device,
    pin: Option<Address>,
    own_name: &str,
) -> bluer::Result<bool> {
    if let Some(mac) = pin {
        // Pinned: the MAC is the whole match; ignore everything else.
        return Ok(device.address() == mac);
    }
    // Unpinned: first FTMS advertiser wins, except our own App Endpoint (which
    // exposes FTMS on the other adapter and is heard over the air).
    // Late-arriving UUIDs are handled by discover_devices_with_changes (see
    // discover()), which re-checks on each PropertyChanged.
    let advertises_ftms = device
        .uuids()
        .await?
        .is_some_and(|uuids| uuids.contains(&ftms::FTMS_SERVICE));
    if !advertises_ftms {
        return Ok(false);
    }
    let is_own = device.alias().await? == own_name;
    Ok(!is_own)
}

/// Connect, subscribe, and pump treadmill notifications until the streams end
/// (disconnect). Always feeds speed to the ANT core; when `bridge` is present,
/// also proxies read characteristics and forwards all notify frames verbatim.
async fn serve(
    core: &Arc<Mutex<SdmCore>>,
    device: &Device,
    bridge: Option<&AppBridge>,
) -> bluer::Result<()> {
    if !device.is_connected().await? {
        device.connect().await?;
    }

    if let Some(bridge) = bridge {
        proxy_reads(device, bridge).await?;
    }

    // Build a tagged, merged stream of every characteristic we care about.
    // Measurement is always present (speed -> ANT). Status and Training Status
    // are only wired when forwarding, and only if the treadmill exposes them.
    let mut streams = Vec::new();
    let measurement = find_char(device, ftms::TREADMILL_MEASUREMENT)
        .await?
        .ok_or_else(|| bluer::Error {
            kind: bluer::ErrorKind::NotFound,
            message: "Treadmill Measurement characteristic (2ACD) not found".into(),
        })?;
    streams.push(tag(measurement.notify().await?, FwdChar::Measurement));

    // Control Point write handle for forwarding app commands to the treadmill.
    let mut control_point = None;
    if bridge.is_some() {
        for (uuid, tag_kind) in [
            (ftms::FITNESS_MACHINE_STATUS, FwdChar::Status),
            (ftms::TRAINING_STATUS, FwdChar::TrainingStatus),
            (ftms::FITNESS_MACHINE_CONTROL_POINT, FwdChar::ControlPoint),
        ] {
            if let Some(ch) = find_char(device, uuid).await? {
                // The treadmill's Control Point indications (responses to our
                // forwarded writes) flow back to the app like any other frame.
                streams.push(tag(ch.notify().await?, tag_kind));
                if tag_kind == FwdChar::ControlPoint {
                    control_point = Some(ch);
                }
            } else {
                tracing::info!(?uuid, "treadmill lacks characteristic; not forwarded");
            }
        }
    }

    // Lock the command receiver for the whole session; only the Link consumes
    // it, and no command is ever enqueued while disconnected (the router rejects
    // those at the App Endpoint), so nothing queues up between sessions.
    let mut commands = match bridge {
        Some(b) => Some(b.commands.lock().await),
        None => None,
    };

    let mut frames = std::pin::pin!(select_all(streams));
    core.lock().unwrap().handle(Event::TreadmillConnected);
    if let Some(bridge) = bridge {
        bridge.connected.store(true, Ordering::Relaxed);
    }
    tracing::info!(addr = %device.address(), forwarding = bridge.is_some(), "treadmill connected");

    loop {
        tokio::select! {
            // A frame from the treadmill: drive ANT and forward to apps.
            frame = frames.next() => {
                let Some((tag_kind, bytes)) = frame else {
                    break; // all notify streams ended: treadmill disconnected
                };
                if tag_kind == FwdChar::Measurement {
                    if let Some(mps) = ftms::instantaneous_speed_mps(&bytes) {
                        core.lock().unwrap().handle(Event::SpeedUpdated(mps));
                    }
                }
                if let Some(bridge) = bridge {
                    // Ignore send errors: no subscriber yet is normal, not fatal.
                    let _ = bridge.frames.send(ForwardFrame { char: tag_kind, bytes });
                }
            }
            // A Control Command from an app: forward verbatim to the treadmill.
            cmd = recv_command(commands.as_mut()) => {
                let Some(bytes) = cmd else { continue };
                match &control_point {
                    Some(cp) => {
                        if let Err(e) = cp.write(&bytes).await {
                            tracing::warn!("Control Point write to treadmill failed: {e}");
                        }
                    }
                    // Connected but the treadmill has no Control Point (rare).
                    // Reject back to the app rather than dropping silently, so
                    // its write reports failure instead of timing out.
                    None => {
                        tracing::warn!("treadmill exposes no Control Point; rejecting command");
                        if let Some(bridge) = bridge {
                            let op = bytes.first().copied().unwrap_or(0);
                            let _ = bridge.frames.send(ForwardFrame {
                                char: FwdChar::ControlPoint,
                                bytes: command::reject(op),
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Await the next forwarded Control Command, or never resolve when there is no
/// command channel (Degraded Mode), so it can sit inertly in a `select!`.
async fn recv_command(
    commands: Option<&mut tokio::sync::MutexGuard<'_, tokio::sync::mpsc::Receiver<Vec<u8>>>>,
) -> Option<Vec<u8>> {
    match commands {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Read the treadmill's Fitness Machine Feature and supported range
/// characteristics; store real values where present, leave fallbacks otherwise.
async fn proxy_reads(device: &Device, bridge: &AppBridge) -> bluer::Result<()> {
    let feature = read_opt(device, ftms::FITNESS_MACHINE_FEATURE).await?;
    let speed = read_opt(device, ftms::SUPPORTED_SPEED_RANGE).await?;
    let incline = read_opt(device, ftms::SUPPORTED_INCLINATION_RANGE).await?;

    // Reset to fallback first so a treadmill lacking a characteristic never
    // inherits the previous treadmill's proxied value.
    let mut reads = bridge.reads.lock().await;
    *reads = ProxiedReads::default();
    if let Some(v) = feature {
        reads.feature = v;
    }
    if let Some(v) = speed {
        reads.speed_range = v;
    }
    if let Some(v) = incline {
        reads.incline_range = v;
    }
    Ok(())
}

/// Read a treadmill characteristic if it exists, keeping the App Endpoint's
/// fallback (returning `None`) when it is absent or unreadable.
async fn read_opt(device: &Device, uuid: Uuid) -> bluer::Result<Option<Vec<u8>>> {
    let Some(ch) = find_char(device, uuid).await? else {
        tracing::info!(?uuid, "treadmill lacks characteristic; keeping fallback");
        return Ok(None);
    };
    match ch.read().await {
        Ok(bytes) => {
            tracing::info!(?uuid, "proxying treadmill value");
            Ok(Some(bytes))
        }
        Err(e) => {
            tracing::warn!(?uuid, "read failed, keeping fallback: {e}");
            Ok(None)
        }
    }
}

/// Tag a raw notification stream with the characteristic it came from so
/// several can be merged into one.
fn tag(
    stream: impl futures_util::Stream<Item = Vec<u8>> + Send + 'static,
    kind: FwdChar,
) -> std::pin::Pin<Box<dyn futures_util::Stream<Item = (FwdChar, Vec<u8>)> + Send>> {
    Box::pin(stream.map(move |bytes| (kind, bytes)))
}

async fn find_char(
    device: &Device,
    uuid: Uuid,
) -> bluer::Result<Option<bluer::gatt::remote::Characteristic>> {
    for service in device.services().await? {
        if service.uuid().await? != ftms::FTMS_SERVICE {
            continue;
        }
        for ch in service.characteristics().await? {
            if ch.uuid().await? == uuid {
                return Ok(Some(ch));
            }
        }
    }
    Ok(None)
}
