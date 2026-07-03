use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;

mod ant;
mod ftms;
mod link;
mod sdm;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_ANT_DEVICE_NUMBER: u16 = 12345;

fn parse_ant_device_number(raw: Option<&str>) -> Result<u16, String> {
    match raw {
        None => Ok(DEFAULT_ANT_DEVICE_NUMBER),
        Some(s) => s
            .trim()
            .parse()
            .map_err(|e| format!("invalid ANT_DEVICE_NUMBER {s:?}: {e}")),
    }
}

/// Optional treadmill MAC pin. `None` means "first FTMS device found wins".
fn parse_treadmill_mac(raw: Option<&str>) -> Result<Option<bluer::Address>, String> {
    match raw {
        None => Ok(None),
        Some(s) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|e| format!("invalid BLEBRIDGE_TREADMILL_MAC {s:?}: {e}")),
    }
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "info (default)".into());
    tracing::info!(version = VERSION, rust_log, "blebridge starting");

    let device_number = parse_ant_device_number(env_opt("ANT_DEVICE_NUMBER").as_deref())
        .unwrap_or_else(|e| {
            tracing::error!(e);
            std::process::exit(1);
        });
    let treadmill_mac = parse_treadmill_mac(env_opt("BLEBRIDGE_TREADMILL_MAC").as_deref())
        .unwrap_or_else(|e| {
            tracing::error!(e);
            std::process::exit(1);
        });

    // The core is shared: the Treadmill Link feeds it connect/speed/disconnect
    // events, the ANT Broadcaster reads it on each TX slot. Both hold the lock
    // only briefly, never across an await.
    let core = Arc::new(Mutex::new(sdm::SdmCore::new()));

    // ANT Broadcaster owns its own thread (blocking USB loop, -> !).
    let ant_core = Arc::clone(&core);
    std::thread::spawn(move || ant::run(ant_core, device_number));

    // Treadmill Link runs on this thread's tokio runtime. Any unexpected BLE
    // error propagates here and exits nonzero (crash-only; Docker restarts us).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            tracing::error!("failed to start tokio runtime: {e}");
            std::process::exit(1);
        });
    if let Err(e) = runtime.block_on(link::run(core, treadmill_mac)) {
        tracing::error!("Treadmill Link failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_semver_like() {
        let parts: Vec<_> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "version {VERSION} is not MAJOR.MINOR.PATCH");
        for part in parts {
            part.parse::<u64>().expect("non-numeric version component");
        }
    }

    #[test]
    fn ant_device_number_defaults_and_parses() {
        assert_eq!(parse_ant_device_number(None), Ok(12345));
        assert_eq!(parse_ant_device_number(Some("777")), Ok(777));
        assert_eq!(parse_ant_device_number(Some(" 777 ")), Ok(777));
        assert!(parse_ant_device_number(Some("nope")).is_err());
        assert!(parse_ant_device_number(Some("70000")).is_err());
    }

    #[test]
    fn treadmill_mac_optional_and_parses() {
        assert_eq!(parse_treadmill_mac(None), Ok(None));
        let mac = "C1:5C:7A:44:82:BA".parse::<bluer::Address>().unwrap();
        assert_eq!(
            parse_treadmill_mac(Some("C1:5C:7A:44:82:BA")),
            Ok(Some(mac))
        );
        assert_eq!(
            parse_treadmill_mac(Some(" C1:5C:7A:44:82:BA ")),
            Ok(Some(mac))
        );
        assert!(parse_treadmill_mac(Some("not-a-mac")).is_err());
    }
}
