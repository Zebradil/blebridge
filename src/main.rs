use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "info (default)".into());
    tracing::info!(version = VERSION, rust_log, "blebridge starting");
    // ponytail: no subsystems yet; BLE/ANT slices land in later issues.
    tracing::info!("nothing to bridge yet, exiting");
}

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver_like() {
        let parts: Vec<_> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "version {VERSION} is not MAJOR.MINOR.PATCH");
        for part in parts {
            part.parse::<u64>().expect("non-numeric version component");
        }
    }
}
