# CONTEXT

Glossary of domain terms for blebridge. Terms here are canonical — code, docs, and discussion use them exactly.

## Terms

**Treadmill** — the physical walking pad exposing a BLE FTMS (Fitness Machine Service) peripheral. The bridge is its client.

**Watch** — a Garmin device receiving treadmill metrics over ANT+. Pairs with the bridge as a Stride & Distance Sensor (device type 124).

**Mobile App** — a fitness app (phone/tablet) that connects to the bridge's BLE peripheral to read metrics and send control commands (speed/incline).

**Bridge** — this application. Simultaneously: BLE central toward the Treadmill, BLE peripheral toward Mobile Apps, ANT+ master toward Watches.

**Treadmill Link** — the bridge's BLE central role: scans for, connects to, and subscribes to the Treadmill's FTMS characteristics.

**App Endpoint** — the bridge's BLE peripheral role: advertises as a virtual treadmill (`BLE_Bridge_Treadmill`) for Mobile Apps.

**ANT Broadcaster** — the bridge's ANT+ master role: broadcasts Stride & Distance Sensor data pages via USB ANT+ stick.

**Degraded Mode** — runtime state when only one Bluetooth adapter is present: Treadmill Link and ANT Broadcaster run, App Endpoint is disabled. Logged clearly at startup; not an error.

**Control Command** — a write from a Mobile App to the FTMS Control Point, forwarded by the bridge to the Treadmill (e.g. set speed, set incline).

**Idle** — Bridge state when no Treadmill is connected. The Treadmill Link keeps a continuous discovery session open (connects within seconds of the Treadmill powering on). The ANT Broadcaster is deliberately silent so the sensor's absence on the Watch signals "treadmill not connected" — silence is a diagnostic, not a fault.
