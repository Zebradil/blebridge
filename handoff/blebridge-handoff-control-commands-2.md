# Handoff #2: blebridge Control Commands — root cause found, two blockers

Continues `/tmp/blebridge-handoff-control-commands.md` (handoff #1). That doc's
hypotheses are now **resolved** — read this one first.

## TL;DR

The "control doesn't move the belt" symptom split into **two independent causes**:

1. **BlueZ 5.55 write-with-response bug** (bridge/host side) — FIXED via workaround.
2. **Treadmill gates FTMS control on console standby state** (treadmill side) — needs a physical retest, not yet confirmed working.

Neither is a routing/gating bug in our Rust code. The command path itself is correct.

## State

- Branch `feat/control-commands`, PR https://github.com/Zebradil/blebridge/pull/11
- Working tree has **uncommitted** changes (see "Uncommitted diff" below). Deployed
  binary on the Pi == current working tree (multi-thread + write_without_response + trace logs).
- `cargo test` = 27 pass. `cargo build` clean.

## Cause 1 — BlueZ 5.55 write-with-response regression (RESOLVED via workaround)

Proven on hardware with `bleak` + `btmon` + `dbus-monitor`:
- App writes 2AD9 (Write **Request**, with response) → our bluer `WriteValue` handler
  replies **success in ~1.9ms** (dbus-monitor) and the byte **reaches the treadmill**
  (`cp.write OK`, Write Command on hci2)…
- …yet bluez sends the app **ATT `0x0E` Unlikely Error ~5.3s later** (btmon).
- Reads work (issue #6) because their reply is non-empty; the empty write-reply is what
  5.55 mishandles. Independent of: the indicate flag, tokio runtime threads, and
  central-side activity (verified each by isolated redeploy).

Known BlueZ regression, 5.55+ (fixed ≥5.56, absent in 5.50):
[bluez/bluez#317](https://github.com/bluez/bluez/issues/317),
[gobbledegook#12](https://github.com/nettlep/gobbledegook/issues/12).

**Workaround applied:** added `write_without_response: true` to the 2AD9
`CharacteristicWrite` in `src/app_endpoint.rs`. Verified: ATT **Write Command** (no
response) → **OK, forwards to treadmill**; Write Request still `0x0E` (unavoidable on 5.55).
2AD9 now advertises `['write-without-response', 'write', 'indicate']`.

## Cause 2 — treadmill won't grant control unless in standby (NOT a bridge bug)

`tools/control_treadmill.py` (new, in repo) sent Request Control (0x00); the treadmill
**never sent a Control Point indication** — `btmon` counted **0** `Handle Value
Indication (0x1d)` frames in a full capture; only measurement notifications
(handle 0x0033). So the treadmill accepts the ATT write but does **not** respond or
grant control in its current console state.

This matches FitShow's message: *"For safety, the treadmill must be in standby before
connecting to the app to enable control."* Belt-running → speed won't change;
belt-stopped → won't start. Treadmill firmware interlock, independent of BlueZ version.

## THE PENDING TEST (do this first)

Physical, user-only. Put the treadmill **console into standby / app-control mode**
(powered, idle, possibly a Bluetooth/app menu on the console), THEN:

```bash
cd ~/code/github.com/zebradil/blebridge
uv run --no-project --with bleak python tools/control_treadmill.py   # observe only, no belt motion
```

Look for `Response to RequestControl -> Success`. If it appears, control is reachable →
proceed to `--speed` / `--start` (MOVES BELT, be on it). If still nothing or
`Control Not Permitted`, the treadmill won't grant control this way — investigate the
console's app-mode entry, or capture what FitShow does differently (btmon on hci0 while
FitShow connects, compare its exact write sequence + any CCC/2ADA/2AD3 subscriptions).

## Uncommitted diff (decide before committing)

- `src/app_endpoint.rs`: **`write_without_response: true`** (the real fix, keep) + trace
  logs in the write closure (keep — control path was flying blind).
- `src/link.rs`: trace logs on forward + `cp.write` Ok/Err (keep).
- `src/main.rs` + `Cargo.toml`: **multi-thread tokio runtime** — was a FALSE hypothesis,
  did NOT fix anything. **Candidate to revert** (original author chose current_thread
  deliberately; mem_limit 64m). Reverting needs a re-test that write-without-response
  still works (it will — the fix is the flag, not threading).
- `tools/control_treadmill.py`: new laptop control tool (keep).
- `compose.rust.yaml`: back to normal (a temporary bogus `BLEBRIDGE_TREADMILL_MAC`
  diagnostic was added then reverted via `git checkout`).

## Open decisions

1. **2AD9 declaration:** keep `write + write-without-response` (current, safe — apps using
   Write Command work; Write-Request apps still `0x0E` but the byte still reaches the belt)
   **vs** `write-without-response` only (forces all apps onto the working path, guarantees
   no `0x0E`; small risk a strict app refuses a CP without the Write property). Decide after
   the standby retest tells us how the real apps (FitShow/Kinomap) behave.
2. **BlueZ upgrade** (optional): Pi is Raspberry Pi OS **Bullseye**, bluez **5.55**. Bookworm
   ships 5.66, or compile ≥5.56 from source on the Pi. Only removes the workaround need —
   **does NOT fix Cause 2**. Low priority.
3. Revert multi-thread runtime? (see above).

## Environment / repro cheatsheet

- Deploy: `mise run deploy-rust` (nix pkgsCross → tar → Pi via `DOCKER_HOST` in `.envrc`).
- Logs: `DOCKER_HOST=ssh://suok@192.168.0.20 docker compose -f compose.rust.yaml logs --since 60s`.
- Pi adapters: hci0 = onboard = App Endpoint (peripheral); hci2 = ASUS BT500 = Link (central).
  Treadmill FTMS MAC `C1:5C:7A:44:82:BA`. App Endpoint BLE addr seen as `B8:27:EB:2D:30:3C`.
- Laptop: single adapter hci0; `bleak` via `uv run --no-project --with bleak`.
- Pi has passwordless sudo; `btmon` + `dbus-monitor` usable. **Persistent systemd/host
  config changes are DENIED by the tool classifier** — don't try `bluetoothd -d` drop-ins.
- Throwaway `bleak` probes from this session lived in the session scratchpad
  (`/tmp/claude-1000/.../scratchpad/probe*.py`) — may not persist; `tools/control_treadmill.py`
  supersedes them.

## Suggested skills

- `/verify` or `/run` — drive `tools/control_treadmill.py` and observe belt + logs after the
  standby retest.
- `/diagnosing-bugs` — if Cause 2 persists in standby (compare FitShow's btmon trace).
- `/ponytail-review` or `/simplify` — before committing, decide multi-thread revert and trim
  trace-log verbosity.
- `/caveman-commit` — for the fix commit (subject e.g. `fix(app): work around BlueZ 5.55
  write-with-response 0x0E on Control Point`).
