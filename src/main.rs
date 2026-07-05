use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;

mod ant;
mod app_endpoint;
mod command;
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

/// Assign one adapter to the Treadmill Link and (if a suitable second exists) one
/// to the App Endpoint. Overrides win when set. Otherwise, `advertisers` (the
/// adapters that actually registered a probe advertisement) drives selection: the
/// Link is the critical path (central -> ANT to the watch) so it gets first pick
/// of a working radio; the App Endpoint needs a *different* radio that can
/// advertise. Adapters that can't advertise (e.g. counterfeit CSR dongles) are
/// skipped for the App and only used for the Link as a last resort. Name order
/// keeps the result deterministic across restarts.
fn assign_adapters(
    mut names: Vec<String>,
    link_override: Option<&str>,
    app_override: Option<&str>,
    advertisers: &HashSet<String>,
) -> Result<Assignment, String> {
    names.sort();
    if names.is_empty() {
        return Err("no Bluetooth adapters found".into());
    }
    let has = |n: &str| names.iter().any(|a| a == n);

    if let Some(n) = link_override {
        if !has(n) {
            return Err(format!(
                "BLEBRIDGE_LINK_ADAPTER {n:?} not found in {names:?}"
            ));
        }
    }
    if let Some(n) = app_override {
        if !has(n) {
            return Err(format!(
                "BLEBRIDGE_APP_ADAPTER {n:?} not found in {names:?}"
            ));
        }
    }
    // One radio can't do both roles. Pinning both to the same adapter is a
    // config error, not a silent Degraded-Mode fallback.
    if let (Some(l), Some(a)) = (link_override, app_override) {
        if l == a {
            return Err(format!(
                "BLEBRIDGE_LINK_ADAPTER and BLEBRIDGE_APP_ADAPTER both set to {l:?}; they need distinct adapters"
            ));
        }
    }

    let app_fixed = app_override.map(str::to_string);
    let not_app = |n: &str| Some(n) != app_fixed.as_deref();

    // Link: honor the override; else a working radio that isn't the pinned App
    // adapter, preferring an advertise-capable one, falling back to any adapter.
    let link = match link_override {
        Some(n) => n.to_string(),
        None => names
            .iter()
            .find(|n| advertisers.contains(*n) && not_app(n))
            .or_else(|| names.iter().find(|n| not_app(n)))
            .or_else(|| names.first())
            .expect("names is non-empty")
            .clone(),
    };

    // App Endpoint: honor the override (unless it collides with the Link); else
    // the first advertise-capable adapter distinct from the Link. `None` disables
    // it — Degraded Mode: a single usable radio, or no adapter can advertise.
    let app = match app_fixed {
        Some(a) => (a != link).then_some(a),
        None => names
            .iter()
            .find(|n| advertisers.contains(*n) && **n != link)
            .cloned(),
    };

    Ok(Assignment { link, app })
}

/// Probe whether an adapter can actually register a BLE advertisement. The mgmt
/// "supported settings" can advertise-claim on controllers that then reject the
/// HCI command (seen on counterfeit CSR8510 dongles), so the only reliable check
/// is to try. Registers a throwaway advertisement; dropping the handle (the
/// returned value is never bound) unregisters it immediately — a sub-second blip.
async fn can_advertise(adapter: &bluer::Adapter) -> bool {
    if adapter.set_powered(true).await.is_err() {
        return false;
    }
    let probe = bluer::adv::Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        discoverable: Some(false),
        local_name: Some("blebridge-probe".into()),
        ..Default::default()
    };
    adapter.advertise(probe).await.is_ok()
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

/// Adapters to exclude entirely, from a comma/space-separated env value. Each
/// entry matches an adapter by hci name (`hci1`) or, preferably, by its stable
/// MAC (`00:1A:7D:DA:71:0B`) — hci indices are assigned by enumeration order and
/// renumber across reboots / USB port swaps, so a MAC survives them. Compared
/// uppercased so MAC case doesn't matter. Empty/absent means skip nothing.
fn parse_skip_list(raw: Option<&str>) -> HashSet<String> {
    raw.into_iter()
        .flat_map(|s| s.split([',', ' ']))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_uppercase)
        .collect()
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

    // No pairing agent on purpose: the App Endpoint refuses pairing
    // (set_pairable(false)) and the treadmill FTMS link needs none, so nothing
    // here ever bonds — registering an auto-accept agent would only contradict
    // that stance.

    // Exclude known-hostile controllers *before* touching them. A counterfeit
    // CSR8510 (0a12:0001) on the Pi can't advertise or scan, and probing its
    // advertising path spews errors into the fragile BlueZ 5.55 daemon. Skip such
    // adapters entirely so we never probe or assign them.
    let skip = parse_skip_list(env_opt("BLEBRIDGE_SKIP_ADAPTERS").as_deref());
    let mut names = Vec::new();
    for name in session.adapter_names().await? {
        let addr = match session.adapter(&name) {
            Ok(a) => a.address().await.ok(),
            Err(_) => None,
        };
        let skipped = skip.contains(&name.to_uppercase())
            || addr
                .as_ref()
                .is_some_and(|a| skip.contains(&a.to_string().to_uppercase()));
        if skipped {
            tracing::info!(
                adapter = name,
                ?addr,
                "skipping adapter (BLEBRIDGE_SKIP_ADAPTERS)"
            );
        } else {
            names.push(name);
        }
    }

    let mut advertisers = HashSet::new();
    for name in &names {
        if let Ok(adapter) = session.adapter(name) {
            if can_advertise(&adapter).await {
                advertisers.insert(name.clone());
            }
        }
    }
    tracing::info!(
        ?names,
        ?advertisers,
        "probed adapter advertising capability"
    );

    let assignment = assign_adapters(
        names,
        env_opt("BLEBRIDGE_LINK_ADAPTER").as_deref(),
        env_opt("BLEBRIDGE_APP_ADAPTER").as_deref(),
        &advertisers,
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
    // Control Command write path: App Endpoint -> Link -> treadmill. The flag
    // lets the App Endpoint reject writes while no treadmill is connected.
    let connected = Arc::new(AtomicBool::new(false));
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let bridge = app_endpoint::AppBridge {
        frames: frames.clone(),
        reads: reads.clone(),
        connected: connected.clone(),
        commands: Arc::new(tokio::sync::Mutex::new(commands_rx)),
    };

    // Both loop forever; select! returns whichever hits a crash-only error first.
    tokio::select! {
        r = app_endpoint::run(app_adapter, ble_name.clone(), frames, reads, commands_tx, connected) => r,
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

    fn adv(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn two_adapters_split_between_roles() {
        // Name order is deterministic regardless of enumeration order.
        let a = assign_adapters(
            names(&["hci1", "hci0"]),
            None,
            None,
            &adv(&["hci0", "hci1"]),
        )
        .unwrap();
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
        let a = assign_adapters(names(&["hci0"]), None, None, &adv(&["hci0"])).unwrap();
        assert_eq!(a.link, "hci0");
        assert_eq!(a.app, None);
    }

    #[test]
    fn no_adapters_errors() {
        assert!(assign_adapters(names(&[]), None, None, &adv(&[])).is_err());
    }

    #[test]
    fn overrides_pick_roles() {
        let a = assign_adapters(
            names(&["hci0", "hci1"]),
            Some("hci1"),
            Some("hci0"),
            &adv(&["hci0", "hci1"]),
        )
        .unwrap();
        assert_eq!(a.link, "hci1");
        assert_eq!(a.app, Some("hci0".into()));
    }

    #[test]
    fn unknown_override_errors() {
        assert!(assign_adapters(names(&["hci0"]), Some("hci9"), None, &adv(&["hci0"])).is_err());
        assert!(assign_adapters(names(&["hci0"]), None, Some("hci9"), &adv(&["hci0"])).is_err());
    }

    #[test]
    fn link_and_app_override_same_adapter_errors() {
        // One radio can't be both roles; pinning both to it is a loud config error.
        assert!(assign_adapters(
            names(&["hci0", "hci1"]),
            Some("hci0"),
            Some("hci0"),
            &adv(&["hci0", "hci1"]),
        )
        .is_err());
    }

    #[test]
    fn skip_list_parses_uppercases_and_ignores_blanks() {
        assert!(parse_skip_list(None).is_empty());
        assert!(parse_skip_list(Some("")).is_empty());
        // Uppercased so a MAC matches regardless of case; names uppercase too.
        assert_eq!(parse_skip_list(Some("hci1")), adv(&["HCI1"]));
        assert_eq!(
            parse_skip_list(Some("00:1a:7d:da:71:0b")),
            adv(&["00:1A:7D:DA:71:0B"])
        );
        assert_eq!(
            parse_skip_list(Some(" hci1 , hci3 ")),
            adv(&["HCI1", "HCI3"])
        );
    }

    #[test]
    fn app_skips_adapter_that_cannot_advertise() {
        // hci1 can't advertise (dead CSR dongle) — App must land on hci2, not hci1.
        let a = assign_adapters(
            names(&["hci0", "hci1", "hci2"]),
            None,
            None,
            &adv(&["hci0", "hci2"]),
        )
        .unwrap();
        assert_eq!(a.link, "hci0");
        assert_eq!(a.app, Some("hci2".into()));
    }

    #[test]
    fn single_advertising_adapter_prioritizes_link() {
        // Only hci0 can advertise; the Link (critical ANT path) gets it, App off.
        let a = assign_adapters(names(&["hci0", "hci1"]), None, None, &adv(&["hci0"])).unwrap();
        assert_eq!(a.link, "hci0");
        assert_eq!(a.app, None);
    }

    #[test]
    fn no_advertising_adapter_still_links() {
        // Nothing can advertise: App off, but the Link still runs on a best-effort
        // adapter so ANT bridging is attempted rather than the process bailing.
        let a = assign_adapters(names(&["hci1"]), None, None, &adv(&[])).unwrap();
        assert_eq!(a.link, "hci1");
        assert_eq!(a.app, None);
    }

    #[test]
    fn app_override_kept_when_auto_link_would_collide() {
        // App pinned to hci0; auto Link must avoid it and take another advertiser.
        let a = assign_adapters(
            names(&["hci0", "hci1", "hci2"]),
            None,
            Some("hci0"),
            &adv(&["hci0", "hci2"]),
        )
        .unwrap();
        assert_eq!(a.link, "hci2");
        assert_eq!(a.app, Some("hci0".into()));
    }
}
