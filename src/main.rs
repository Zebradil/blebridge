use tracing_subscriber::EnvFilter;

mod ant;
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

fn parse_fake_speed(raw: Option<&str>) -> Result<Option<f64>, String> {
    match raw {
        None => Ok(None),
        Some(s) => match s.trim().parse::<f64>() {
            Ok(v) if v >= 0.0 => Ok(Some(v)),
            Ok(_) => Err(format!("invalid BLEBRIDGE_FAKE_SPEED {s:?}: negative")),
            Err(e) => Err(format!("invalid BLEBRIDGE_FAKE_SPEED {s:?}: {e}")),
        },
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
    let fake_speed =
        parse_fake_speed(env_opt("BLEBRIDGE_FAKE_SPEED").as_deref()).unwrap_or_else(|e| {
            tracing::error!(e);
            std::process::exit(1);
        });

    // ponytail: fake event source until the Treadmill Link slice lands;
    // BLEBRIDGE_FAKE_SPEED=<m/s> simulates a treadmill connected at constant speed.
    match fake_speed {
        None => {
            tracing::info!(
                "no treadmill source in this slice; set BLEBRIDGE_FAKE_SPEED=<m/s> to broadcast"
            );
        }
        Some(mps) => {
            tracing::info!(
                mps,
                device_number,
                "broadcasting fake constant speed over ANT+"
            );
            let mut core = sdm::SdmCore::new();
            core.handle(sdm::Event::TreadmillConnected);
            core.handle(sdm::Event::SpeedUpdated(mps));
            ant::run(core, device_number);
        }
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
    fn fake_speed_optional_and_parses() {
        assert_eq!(parse_fake_speed(None), Ok(None));
        assert_eq!(parse_fake_speed(Some("1.5")), Ok(Some(1.5)));
        assert!(parse_fake_speed(Some("fast")).is_err());
        assert!(parse_fake_speed(Some("-1.0")).is_err());
    }
}
