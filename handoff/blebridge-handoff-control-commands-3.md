# Handoff #3: blebridge Control Commands — control WORKS; BlueZ 5.55 is the last blocker

Supersedes `/tmp/blebridge-handoff-control-commands-2.md`. Read this one first.

## TL;DR

`RequestControl -> Success` now verified **end-to-end** (laptop tool → App Endpoint →
Link → treadmill → response back to the laptop). Two code fixes did it. Everything
still flaky traces to BlueZ 5.55 heap-corruption crashes; **the user is doing an OS
upgrade to Bookworm (BlueZ 5.66) themselves** before further debugging.

## The two real bugs (both FIXED in working tree)

1. **`src/link.rs`: bluer's `write()` defaults to ATT Write Command** (`WriteOp::Command`,
   without response). The treadmill (FitShow module `FS-D9D0A1`) ACKs Write Commands at
   ATT level but its FTMS layer silently drops them — logs said "written OK", nothing
   happened, zero indications, ever. Fix: `cp.write_ext(&bytes, &CharacteristicWriteRequest
   { op_type: WriteOp::Request, .. })`. Telltale that it works: write ACK latency went
   from ~4ms (fire-and-forget) to ~130–430ms (real round-trip).
2. **`src/app_endpoint.rs`: BlueZ 5.55's GATT server cannot deliver indications.** The
   app-emitted Value PropertiesChanged on an indicate characteristic never becomes an ATT
   Handle Value Indication; the bluer session dies with "the receiving Bluetooth device
   has stopped the notification session". Verified with dbus-monitor (emission present)
   + btmon on hci0 (0 indications on air). Workaround: 2AD9 is **notify-only** (off-spec;
   FTMS wants indicate). Has a `ponytail:` comment; **revert to indicate after the BlueZ
   upgrade.**

Treadmill-side constraint (not a bug): control granted only in **standby (belt
stopped)** — FitShow enforces the same app-side.

## Remaining flakiness = BlueZ 5.55 heap corruption

- ~1 in 5 RequestControl responses still doesn't reach the client, and multi-command
  runs (`--speed`, `--stop`) sometimes die mid-script with bleak "Service Discovery has
  not been performed yet" (= disconnected).
- Cause chain: whenever the laptop connects, Pi's bluetoothd reverse-probes the
  laptop's GATT db as a client. 5.55 heap-corrupts in that path: first via the midi
  plugin (`malloc(): memory corruption (fast)`), and after disabling midi, via
  `gatt-client.c service_create()` → `corrupted size vs. prev_size in fastbins` → ABRT.
  systemd restarts it (restart.conf drop-in), blebridge self-heals via its
  advertisement-liveness poll, but connections during the ~1 min recovery window fail
  flaky — **just retry, don't debug those.**
- Phones expose less GATT than a bluez laptop, so production is less exposed, but 5.55
  crashes on phone disconnects too (older observation). The upgrade fixes the class.

## Host changes applied (Pi `toddler`, user-authorized)

- `/etc/systemd/system/bluetooth.service.d/noplugin.conf`:
  `ExecStart=/usr/libexec/bluetooth/bluetoothd --noplugin=midi,sap` — raised control
  reliability from ~30% to ~80%. Keep or drop after Bookworm (5.66 probably doesn't
  need it, but midi/sap are useless here anyway).
- `uv` installed at `~/.local/bin/uv` on the Pi; `/tmp/control_treadmill.py` copied
  there. Direct-to-treadmill testing from the Pi:
  `~/.local/bin/uv run --no-project --with bleak python /tmp/control_treadmill.py --address C1:5C:7A:44:82:BA --adapter hci2`
  (bridge container must be stopped to free the treadmill).

## Uncommitted diff (branch `feat/control-commands`, PR #11)

- `src/link.rs` — WriteOp::Request fix (keep, THE fix) + trace logs.
- `src/app_endpoint.rs` — notify-only 2AD9 with ponytail comment (keep until Bookworm,
  then revert to indicate) + `write_without_response: true` from handoff #2 (5.55 0x0E
  workaround; after upgrade, test whether Write Requests get proper replies and drop it)
  + subscribe/notify-failure info logs (keep, cheap).
- `tools/control_treadmill.py` — laptop/Pi control tool; gained `--adapter` and
  scan-by-address.
- Multi-thread tokio runtime from handoff #2 was **reverted** (false hypothesis,
  re-verified: control works on `current_thread`). `Cargo.toml`/`src/main.rs` clean.
- `cargo test` = 27 pass. Deployed binary on Pi == working tree.
- `next-debug-steps.md` in repo root is stale (pre-dates all of this) — delete it.

## After the Bookworm upgrade, in order

1. `mise run deploy-rust`; verify Link reconnects + App Endpoint advertises.
2. Laptop: `uv run --no-project --with bleak python tools/control_treadmill.py` ×5 —
   expect `Response to RequestControl -> Success` 5/5.
3. Revert 2AD9 to indicate (`fwd_notify(..., true)`, drop the ponytail block); retest.
   If indications now deliver, also try dropping `write_without_response: true`.
4. **Belt test** (user on treadmill, belt stopped): `--start`, then `--speed 2.0`, then
   `--stop`. Watch `2ADA` status frames.
5. Real app test: FitShow/Kinomap on the phone (re-enable phone BT — it was disabled for
   testing because a backgrounded app kept auto-connecting and spamming `RequestControl`).
6. Commit (subject idea: `fix(link): send Control Point writes as ATT Write Request` +
   separate commit for the 5.55 workarounds) and update PR #11.

## Environment cheatsheet

- Deploy: `mise run deploy-rust`. Logs: `DOCKER_HOST=ssh://suok@192.168.0.20 docker
  compose -f compose.rust.yaml logs --since 60s`.
- Pi: `ssh suok@192.168.0.20`, passwordless sudo. hci0 = App Endpoint (onboard),
  hci2 = Link (ASUS BT500), hci1 dead (skipped via env). Treadmill FTMS
  `C1:5C:7A:44:82:BA`, name `FS-D9D0A1`. App Endpoint addr `B8:27:EB:2D:30:3C`.
- Debug recipe that cracked it: btmon on hci2 + btmon on hci0 + `dbus-monitor --system`
  simultaneously during one tool run — shows exactly which hop drops a frame. Beware:
  a 20s BLE scan can eat a short btmon window and fake "0 frames".
- Memory files updated: `ftms-control-root-causes.md` (new), `pi-bt-adapters.md`
  (midi crash + noplugin), `MEMORY.md` index.
