use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;

mod ant;
mod app_endpoint;
mod ftms;
mod link;
mod sdm;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_ANT_DEVICE_NUMBER: u16 = 12345;
const DEFAULT_BLE_NAME: &str = "BLE_Bridge_Treadmill";

/// Adapter roles resolved from the available adapters plus optional overrides.
/// `app == None` means Degraded Mode: one adapter, App Endpoint disabled.
#[derive(Debug, PartialEq)]
struct Assignment {
    link: String,
    app: Option<String>,
}

/// Assign one adapter to the Treadmill Link and (if a second exists) one to the
/// App Endpoint. Overrides win when set; otherwise adapters are taken in name
/// order so assignment is deterministic across restarts.
fn assign_adapters(
    mut names: Vec<String>,
    link_override: Option<&str>,
    app_override: Option<&str>,
) -> Result<Assignment, String> {
    names.sort();
    let has = |n: &str| names.iter().any(|a| a == n);

    let link = match link_override {
        Some(n) if has(n) => n.to_string(),
        Some(n) => {
            return Err(format!(
                "BLEBRIDGE_LINK_ADAPTER {n:?} not found in {names:?}"
            ))
        }
        None => names.first().ok_or("no Bluetooth adapters found")?.clone(),
    };

    let app = match app_override {
        Some(n) if has(n) => Some(n.to_string()),
        Some(n) => {
            return Err(format!(
                "BLEBRIDGE_APP_ADAPTER {n:?} not found in {names:?}"
            ))
        }
        // Auto: the App Endpoint takes the first adapter that isn't the Link's.
        None => names.iter().find(|a| **a != link).cloned(),
    };

    Ok(Assignment { link, app })
}

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
    let ble_name = env_opt("BLEBRIDGE_BLE_NAME").unwrap_or_else(|| DEFAULT_BLE_NAME.to_string());

    // The core is shared: the Treadmill Link feeds it connect/speed/disconnect
    // events, the ANT Broadcaster reads it on each TX slot. Both hold the lock
    // only briefly, never across an await.
    let core = Arc::new(Mutex::new(sdm::SdmCore::new()));

    // ANT Broadcaster owns its own thread (blocking USB loop, -> !).
    let ant_core = Arc::clone(&core);
    std::thread::spawn(move || ant::run(ant_core, device_number));

    // BLE roles run on this thread's tokio runtime. Any unexpected BLE error
    // propagates here and exits nonzero (crash-only; Docker restarts us).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            tracing::error!("failed to start tokio runtime: {e}");
            std::process::exit(1);
        });
    if let Err(e) = runtime.block_on(run_ble(core, treadmill_mac, ble_name)) {
        tracing::error!("BLE role failed: {e}");
        std::process::exit(1);
    }
}

/// Resolve adapters, then run the Treadmill Link and (unless Degraded Mode) the
/// App Endpoint concurrently. Returns on the first crash-only error.
async fn run_ble(
    core: Arc<Mutex<sdm::SdmCore>>,
    treadmill_mac: Option<bluer::Address>,
    ble_name: String,
) -> bluer::Result<()> {
    let session = bluer::Session::new().await?;
    let assignment = assign_adapters(
        session.adapter_names().await?,
        env_opt("BLEBRIDGE_LINK_ADAPTER").as_deref(),
        env_opt("BLEBRIDGE_APP_ADAPTER").as_deref(),
    )
    .unwrap_or_else(|e| {
        tracing::error!(e);
        std::process::exit(1);
    });

    let link_adapter = session.adapter(&assignment.link)?;
    let reads = Arc::new(tokio::sync::Mutex::new(
        app_endpoint::ProxiedReads::default(),
    ));

    let Some(app_name) = assignment.app else {
        tracing::warn!(
            adapter = assignment.link,
            "Degraded Mode: only one Bluetooth adapter — App Endpoint disabled, ANT bridging unaffected"
        );
        return link::run(link_adapter, core, treadmill_mac, None, ble_name).await;
    };

    let app_adapter = session.adapter(&app_name)?;
    let (frames, _) = tokio::sync::broadcast::channel(64);
    let bridge = app_endpoint::AppBridge {
        frames: frames.clone(),
        reads: reads.clone(),
    };

    // Both loop forever; select! returns whichever hits a crash-only error first.
    tokio::select! {
        r = app_endpoint::run(app_adapter, ble_name.clone(), frames, reads) => r,
        r = link::run(link_adapter, core, treadmill_mac, Some(bridge), ble_name) => r,
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

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn two_adapters_split_between_roles() {
        // Name order is deterministic regardless of enumeration order.
        let a = assign_adapters(names(&["hci1", "hci0"]), None, None).unwrap();
        assert_eq!(
            a,
            Assignment {
                link: "hci0".into(),
                app: Some("hci1".into())
            }
        );
    }

    #[test]
    fn one_adapter_is_degraded_mode() {
        let a = assign_adapters(names(&["hci0"]), None, None).unwrap();
        assert_eq!(a.link, "hci0");
        assert_eq!(a.app, None);
    }

    #[test]
    fn no_adapters_errors() {
        assert!(assign_adapters(names(&[]), None, None).is_err());
    }

    #[test]
    fn overrides_pick_roles() {
        let a = assign_adapters(names(&["hci0", "hci1"]), Some("hci1"), Some("hci0")).unwrap();
        assert_eq!(a.link, "hci1");
        assert_eq!(a.app, Some("hci0".into()));
    }

    #[test]
    fn unknown_override_errors() {
        assert!(assign_adapters(names(&["hci0"]), Some("hci9"), None).is_err());
        assert!(assign_adapters(names(&["hci0"]), None, Some("hci9")).is_err());
    }
}
