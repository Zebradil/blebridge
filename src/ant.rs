//! ANT Broadcaster adapter: drives the pure [`SdmCore`] over an ANT+ USB
//! stick using the ant-rs message layer and USB driver. Blocking; owns the
//! calling thread.

use std::time::{Duration, Instant};

use ant::drivers::{is_ant_usb_device_from_device, Driver, UsbDriver};
use ant::messages::channel::MessageCode;
use ant::messages::config::{
    AssignChannel, ChannelId, ChannelPeriod, ChannelRfFrequency, ChannelType, DeviceType,
    SetNetworkKey, TransmissionChannelType, TransmissionGlobalDataPages, TransmissionType,
};
use ant::messages::control::{OpenChannel, ResetSystem};
use ant::messages::data::BroadcastData;
use ant::messages::{RxMessage, TransmitableMessage};
use rusb::{Device, DeviceList, GlobalContext};

use crate::sdm::{Event, SdmCore};

// ANT+ protocol constants, not configuration (see PRD).
const NETWORK_KEY: [u8; 8] = [0xB9, 0xA5, 0x21, 0xFB, 0xBD, 0x72, 0xC3, 0x45];
const DEVICE_TYPE: u8 = 124; // Stride & Distance Sensor
const CHANNEL_PERIOD: u16 = 8134; // 32768/8134 ≈ 4.03 Hz
const RF_FREQUENCY: u8 = 57; // 2457 MHz
const CHANNEL: u8 = 0;

const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_CONSECUTIVE_SETUP_FAILURES: u32 = 5;

enum SessionEnd {
    /// No ANT stick on the bus — retried forever (it may get plugged in).
    NoStick,
    /// Stick present but channel setup failed — repeated failures are
    /// unrecoverable and exit the process.
    SetupFailed(String),
    /// Channel was open and broadcasting, then the stick errored/unplugged —
    /// retried forever with capped backoff.
    TxFailed(String),
}

/// Run the ANT Broadcaster until the process ends. Transient errors retry
/// with capped backoff; persistent setup failure exits nonzero (crash-only,
/// Docker restarts us).
pub fn run(mut core: SdmCore, device_number: u16) -> ! {
    let start = Instant::now();
    let mut backoff = Duration::from_secs(1);
    let mut setup_failures = 0u32;

    loop {
        match session(&mut core, device_number, start) {
            SessionEnd::NoStick => {
                // A replugged stick is a fresh start, not a repeat failure.
                setup_failures = 0;
                tracing::warn!("no ANT+ USB stick found, retrying in {backoff:?}");
            }
            SessionEnd::SetupFailed(err) => {
                setup_failures += 1;
                if setup_failures >= MAX_CONSECUTIVE_SETUP_FAILURES {
                    tracing::error!(
                        err,
                        "ANT channel setup failed {setup_failures} times, giving up"
                    );
                    std::process::exit(1);
                }
                tracing::warn!(err, "ANT channel setup failed, retrying in {backoff:?}");
            }
            SessionEnd::TxFailed(err) => {
                setup_failures = 0;
                backoff = Duration::from_secs(1);
                tracing::warn!(err, "ANT session ended, retrying in {backoff:?}");
            }
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn find_stick() -> Option<Device<GlobalContext>> {
    let devices = match DeviceList::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("USB enumeration failed: {e}");
            return None;
        }
    };
    devices.iter().find(is_ant_usb_device_from_device)
}

fn session(core: &mut SdmCore, device_number: u16, start: Instant) -> SessionEnd {
    let Some(device) = find_stick() else {
        return SessionEnd::NoStick;
    };
    let mut driver = match UsbDriver::new(device) {
        Ok(d) => d,
        Err(e) => return SessionEnd::SetupFailed(format!("open usb device: {e:?}")),
    };

    if let Err(e) = open_channel(&mut driver, device_number) {
        return SessionEnd::SetupFailed(e);
    }
    tracing::info!(device_number, "ANT channel open, broadcasting SDM pages");

    loop {
        match driver.get_message() {
            // Driver reads are non-blocking (1 ms USB timeout); nap between
            // polls so the ~4 Hz TX loop doesn't spin a core.
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Ok(Some(msg)) => {
                if let RxMessage::ChannelEvent(ev) = &msg.message {
                    if ev.payload.message_code == MessageCode::EventTx {
                        let timestamp = start.elapsed().as_secs_f64();
                        if let Some(page) = core.handle(Event::TxRequested { timestamp }) {
                            let data = BroadcastData::new(CHANNEL, page);
                            if let Err(e) = driver.send_message(&data) {
                                return SessionEnd::TxFailed(format!("{e:?}"));
                            }
                        }
                    }
                }
            }
            Err(e) => return SessionEnd::TxFailed(format!("{e:?}")),
        }
    }
}

fn open_channel(driver: &mut UsbDriver<GlobalContext>, device_number: u16) -> Result<(), String> {
    fn send(
        driver: &mut UsbDriver<GlobalContext>,
        label: &str,
        msg: &dyn TransmitableMessage,
    ) -> Result<(), String> {
        driver
            .send_message(msg)
            .map_err(|e| format!("{label}: {e:?}"))
    }

    send(driver, "reset", &ResetSystem::new())?;
    // ANT chips need a moment to reboot after ResetSystem.
    std::thread::sleep(Duration::from_millis(500));

    send(driver, "network key", &SetNetworkKey::new(0, NETWORK_KEY))?;
    send(
        driver,
        "assign channel",
        &AssignChannel::new(CHANNEL, ChannelType::BidirectionalMaster, 0, None),
    )?;
    // Transmission type 5 (0b101) matches the Python implementation:
    // independent channel, global data pages used.
    send(
        driver,
        "channel id",
        &ChannelId::new(
            CHANNEL,
            device_number,
            DeviceType::new(DEVICE_TYPE.into(), false),
            TransmissionType::new(
                TransmissionChannelType::IndependentChannel,
                TransmissionGlobalDataPages::GlobalDataPagesUsed,
                0.into(),
            ),
        ),
    )?;
    send(
        driver,
        "channel period",
        &ChannelPeriod::new(CHANNEL, CHANNEL_PERIOD),
    )?;
    send(
        driver,
        "rf frequency",
        &ChannelRfFrequency::new(CHANNEL, RF_FREQUENCY),
    )?;
    send(driver, "open channel", &OpenChannel::new(CHANNEL))?;
    Ok(())
}
