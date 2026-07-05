**Confirmed — zero Control Point indications from the treadmill.** The full capture shows the treadmill sends only measurement notifications (handle 0x0033); it never sends a `0x1d` indication. Our Request Control byte reaches it (Write Command `00` on hci2), but the treadmill **does not respond and does not grant control**.

That's the answer to FitShow. Here's all three of your points:

## 1. FitShow "must be in standby" — this is the treadmill, not the bridge

The treadmill gates FTMS control on its **console state**. In its current state it accepts the write at the BLE layer but ignores it — no response, no speed change. That's exactly what FitShow's message means: **the treadmill console must be in standby/app mode before it will grant app control.**

- Belt running → speed doesn't change: treadmill won't accept control mid-run.
- Belt stopped → won't start + safety message: FitShow knows control isn't granted yet.

The bridge forwards commands correctly; the treadmill is the gate. **Next test: put the console into standby (the state FitShow asks for — powered on, idle, possibly a Bluetooth/app menu on the console), then connect.** When control is granted you'll see a `Response to RequestControl -> Success` line.

## 2. Testing speed change from the laptop

Wrote `tools/control_treadmill.py`. It uses the working write-without-response path and prints the treadmill's control responses.

⚠️ **`--start` and `--speed` MOVE THE BELT. Be on the treadmill, one hand on the rail. Put the console in standby FIRST or control won't be granted.**

```bash
cd ~/code/github.com/zebradil/blebridge

# Observe only — no belt motion. Look for "RequestControl -> Success".
uv run --no-project --with bleak python tools/control_treadmill.py

# Set 3 km/h while belt already running (needs control granted):
uv run --no-project --with bleak python tools/control_treadmill.py --speed 3.0

# Start belt then set speed (from standby):
uv run --no-project --with bleak python tools/control_treadmill.py --start --speed 2.0

# Stop:
uv run --no-project --with bleak python tools/control_treadmill.py --stop
```

If you see `RequestControl -> Success`, control is working and speed should follow. If you see nothing (like just now) or `Control Not Permitted`, the console isn't in the control-granting state.

## 3. BlueZ upgrade

Pi runs Raspberry Pi OS **Bullseye**, bluez **5.55**. The `0x0E` bug is fixed in ≥5.56. Options:
- **OS upgrade Bullseye→Bookworm** → bluez 5.66. Cleanest, but a full headless-Pi OS upgrade (back up the SD card first).
- **Compile bluez ≥5.56 from source** on the Pi, keep the OS. Targeted; ~30 min build. I can give you the exact commands.
- Backports won't help (still 5.55-era).

**But note:** upgrading bluez only removes the need for the write-without-response workaround (lets plain Write-Request apps work). **It will NOT fix the FitShow issue** — that's the treadmill's standby gate, independent of bluez. So the workaround already deployed is enough to unblock control; the upgrade is optional polish.

Sources for the bluez bug: [bluez/bluez#317](https://github.com/bluez/bluez/issues/317), [gobbledegook#12](https://github.com/nettlep/gobbledegook/issues/12)

---

**My recommendation:** first re-run the observe test with the **console in standby** — that tells us whether control is actually reachable on this treadmill at all. Everything else (final 2AD9 declaration, reverting the multi-thread experiment, committing) I'll settle after we confirm the belt responds. Want me to keep the current `write + write-without-response` declaration, and shall I hand you the bluez-from-source commands?
