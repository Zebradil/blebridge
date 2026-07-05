# Handoff: blebridge Control Commands — write path not moving belt

## State

Issue #7 (Control Commands: mobile app controls treadmill) implemented and merged-ready.

- Branch: `feat/control-commands`, commit `02d859b`
- PR: https://github.com/Zebradil/blebridge/pull/11 (CI green, `closes #7`)
- Issue: `gh issue view 7`
- Design: `docs/adr/0002-raw-ftms-passthrough.md` (verbatim passthrough + the synthesize-on-unreachable carve-out)

Code lives in: `src/command.rs` (pure route/reject seam + tests), `src/app_endpoint.rs` (`control_point_char`, exposes 2AD9 Write+Indicate), `src/link.rs` (`serve()` select loop forwards commands to treadmill 2AD9, forwards treadmill indications back), `src/main.rs` (mpsc command channel + `connected` AtomicBool). `cargo test` = 27 pass.

## The problem (why this handoff)

Deployed to Pi, logs look right (App Endpoint advertising, treadmill connected forwarding=true). **Control does not move the belt:**

- **nRF Connect**: user couldn't send a write to 2AD9 at all (unclear if characteristic showed writable, or UI issue).
- **Kinomap**: no speed-control feature shown in the app.
- **FitShow**: speed control had no effect on the belt.

So end-to-end write path is unverified and currently appears broken. Root cause unknown — it was never validated on hardware (only unit-tested; BLE plumbing isn't unit-testable).

## Top hypotheses, most likely first

1. **Apps hide speed control because the proxied Fitness Machine Feature (2ACC) lacks Target-Setting bits.**
   FTMS apps gate the control UI on the *Target Setting Features* field (2nd uint32 of 2ACC): bit 0 = Speed Target Setting Supported, bit 1 = Inclination. `src/link.rs::proxy_reads` proxies the treadmill's REAL 2ACC verbatim. If the treadmill doesn't advertise those bits, Kinomap/FitShow won't offer speed control — matches "Kinomap has no speed control." The hardcoded fallback (`ftms::FALLBACK_FEATURE = 0D 16 00 00 03 00 00 00`) DOES set speed+incline (0x03), but the fallback is only used when the treadmill lacks 2ACC — a treadmill that HAS a poor 2ACC overrides it.
   → **Check the logged proxied 2ACC value** (`mise run logs`, look for "proxying treadmill value" on 2ACC / read it via nRF Connect). If Target-Setting bits are clear, decide: force the feature bits (override proxy for 2ACC's 2nd dword) so apps expose control. This is the most probable fix.

2. **No logging on the write path — currently flying blind.**
   `control_point_char`'s write handler logs nothing on the Forward branch; `link.rs` logs only write *failures*. Can't tell if the app's write even reaches the callback, or if the forward to the treadmill succeeds.
   → **Add trace logs first** (cheap, do before anything else): in `app_endpoint.rs` write closure log `value` bytes + route decision; in `link.rs` command branch log forwarded bytes + `cp.write` Ok/Err. Redeploy, then every test tells you exactly where the chain breaks.

3. **FTMS handshake not completing (indication path).** Apps write Request Control (0x00) and expect indication `80 00 01` before offering control. If 2AD9 indications aren't reaching the app (CCC not wired, or treadmill 2AD9 not subscribed), apps silently give up. Verify the treadmill actually exposes 2AD9 and that the Link subscribed (log: "treadmill lacks characteristic; not forwarded" would show if 2AD9 is missing on the treadmill — that alone would explain everything: connected-but-no-Control-Point path only *rejects*, never moves the belt).
   → **Confirm the real treadmill even has a writable 2AD9.** If it doesn't, the whole feature is moot for this device and the belt can only be driven the way it already is.

4. **bluer didn't expose 2AD9 as writable to the app** (nRF couldn't write). Verify the advertised characteristic properties from a scanner. `CharacteristicWrite { write: true }` maps to BlueZ "write" (Write Request). Some apps use Write-Without-Response — consider also setting `write_without_response: true`.

## Suggested next step: drive 2AD9 from the laptop (bypass phone apps)

Phone apps add too many unknowns. Script the control write from the laptop with `bleak` (Python; repo already has a Python env / `.venv`). Connect to `BLE_Bridge_Treadmill`, enable notify on 2AD9, write `00` (Request Control), `07` (Start), then `02 <speed LE, 0.01 km/h>` (e.g. `02 2C 01` = 3.00 km/h). Observe indications + Pi logs + belt. This isolates bridge-vs-app cleanly and gives the exact byte-level trace. Note: laptop BLE central + the Pi's App Endpoint — the memory `pi-bt-adapters` has a laptop-loopback test recipe worth reusing. Watch for adapter conflicts if testing from the same host that talks to the treadmill.

## Watch out for

- **Degraded Mode**: if only one Pi adapter comes up, App Endpoint is OFF and 2AD9 doesn't exist. Confirm logs say `forwarding=true`, not `Degraded Mode`.
- **Connected-but-no-Control-Point** now *rejects* (synthesized `80 <op> 04`) rather than dropping — if the treadmill has no 2AD9, every app write comes back as Operation Failed. That's correct behavior but looks like "control doesn't work."
- Treadmill FTMS pinned MAC ends `...82:BA` (memory `pi-bt-adapters`).

## Suggested skills

- `/diagnosing-bugs` — structured root-cause loop for the "controls don't work" symptom; start here.
- `/verify` or `/run` — drive the laptop `bleak` control-write test and observe belt + logs.
- `/caveman-commit` — for any fix commit.
