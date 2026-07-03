# Rewrite the bridge in Rust

The Python implementation worked but the stack was disliked for building, deploying, and CI (no CI existed; none was wanted for Python). The observed pains — slow treadmill→watch propagation and slow Garmin sensor discovery — were traced to design flaws (exponential scan backoff to 60s; no ANT payload while paused), not to Python itself, and the rewrite must fix them by design: event-driven architecture (tokio tasks + watch/mpsc channels, no polling tick) with a continuous BLE discovery session.

Decisions bundled with the rewrite:

- **Stack**: `bluer` for both BLE roles (only maintained Rust crate with GATT client + server on Linux; same BlueZ D-Bus underneath). `ant-rs` (`messages` + `drivers::usb` layers only) for ANT+; it is a young single-maintainer crate, so the fallback is to vendor/fork it — the consumed surface is ~900 lines.
- **Scope**: PyQt5 GUI dropped; core exposes state via watch channel + command channel so a future UI can attach. Single-adapter machines run Degraded Mode (see CONTEXT.md) instead of being unsupported.
- **Deployment**: Docker stays (owner preference over static-binary + systemd), but the image shrinks to a static musl binary; D-Bus socket mount and USB passthrough as before. CI on GitHub Actions: fmt/clippy/test on PR, multi-arch image on tag.
- **Failure handling**: crash-only. Subsystems retry transient errors with capped backoff; anything unexpected exits the process and Docker restarts it. Replaces in-process adapter power-cycle hacks.
- **Config**: env vars with defaults, no config file. Speed/incline ranges are proxied from the treadmill's own FTMS range characteristics instead of being configured.
- **Migration**: Rust lands in the same repo at the root; Python remains until one real walk session validates the Rust bridge end-to-end (watch records distance, mobile app controls speed), then is deleted.
