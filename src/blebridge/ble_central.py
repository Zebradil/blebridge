"""Example of how to create a Central device/GATT Client"""
import struct
import time
import threading
import subprocess
import types

from bluezero import adapter
from bluezero import central
from bluezero import constants
from bluezero import dbus_tools


def central_handler(ftms_monitor):
    ftms_monitor.run()


class BleCentral:
    def __init__(self, stop_event=None, adapter_address=None, **kwargs):
        self.stop_event = stop_event
        self.adapter_address = adapter_address
        self.blacklist_address = kwargs.get('blacklist_address', None)
        self.central_thread = None
        self._dongle = None

        self.ftms_srv = '00001826-0000-1000-8000-00805f9b34fb'
        self.tm_data_uuid = '00002acd-0000-1000-8000-00805f9b34fb'
        self.fm_data_uuid = '00002ada-0000-1000-8000-00805f9b34fb'
        self.ts_data_uuid = '00002ad3-0000-1000-8000-00805f9b34fb'
        self.ftms_ctr_pt_uuid = '00002ad9-0000-1000-8000-00805f9b34fb'

        self.connected = False
        self.values = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        self.value = struct.pack('<BBHHBHHHHBBH', 140, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        self.ftms_status_value = struct.pack('<B', 0)
        self.training_status_value = struct.pack('<B', 0)
        self.ftms_control_value = [False, True, struct.pack('<B', 1)]

    def _get_dongle(self):
        if self._dongle is None:
            self._dongle = adapter.Adapter(adapter_addr=self.adapter_address)
        return self._dongle

    def ble_central_start(self):
        backoff = 5
        while not self.stop_event.is_set():
            try:
                dongle = self._get_dongle()
                if not dongle.powered:
                    dongle.powered = True

                print("scanning")
                devices = list(self.scan_for_ftms())

                if not devices:
                    print(f"No FTMS found. Retrying in {backoff}s...")
                    self.stop_event.wait(backoff)
                    backoff = min(backoff * 2, 60)
                    continue

                backoff = 5
                dev = devices[0]
                print("FTMS Measurement Device Found!", dev.alias)
                self.connect_and_run(dev)

                # connect_and_run returned — disconnect happened
                print("Connection lost, will retry...")

            except Exception as e:
                print(f"ble_central_start error ({type(e).__name__}: {e})")
                self.stop_event.wait(backoff)
                backoff = min(backoff * 2, 60)
            finally:
                self.connected = False
                self.values = [0] * 10

    def _reset_adapter(self, device_address=None):
        """Reset BLE adapter state for clean reconnection."""
        try:
            dongle = self._get_dongle()
            if device_address:
                subprocess.run(['bluetoothctl', 'remove', device_address],
                               capture_output=True, timeout=10)
            dongle.powered = False
            time.sleep(0.5)
            dongle.powered = True
            time.sleep(1)
        except Exception as e:
            print(f"Adapter reset error: {e}")
            time.sleep(2)

    def connect_and_run(self, dev):
        """Connect to a single FTMS device and monitor until disconnect."""
        dev_address = dev.address
        try:
            dongle = self._get_dongle()
            if not dongle.powered:
                dongle.powered = True

            monitor = central.Central(
                adapter_addr=self.adapter_address,
                device_addr=dev_address)

            measurement_char_tm = monitor.add_characteristic(self.ftms_srv, self.tm_data_uuid)
            measurement_char_fm = monitor.add_characteristic(self.ftms_srv, self.fm_data_uuid)
            measurement_char_ts = monitor.add_characteristic(self.ftms_srv, self.ts_data_uuid)
            control_point_char = monitor.add_characteristic(self.ftms_srv, self.ftms_ctr_pt_uuid)

            print("Connecting to " + dev.alias)
            monitor.connect()

            connect_attempts = 0
            while not monitor.connected and connect_attempts < 10:
                time.sleep(3)
                if not monitor.connected:
                    monitor.connect()
                connect_attempts += 1

            if not monitor.connected:
                print("Didn't connect to device!")
                return

            print("BLE_central connected")
            print(time.time())
            self.connected = True

            # Enable notifications
            measurement_char_tm.start_notify()
            measurement_char_tm.add_characteristic_cb(self.on_new_ftms_measurement)
            measurement_char_fm.start_notify()
            measurement_char_fm.add_characteristic_cb(self.on_new_fm_measurement)
            measurement_char_ts.start_notify()
            measurement_char_ts.add_characteristic_cb(self.on_new_ts_measurement)

            self.central_thread = threading.Thread(target=central_handler, args=(monitor,))
            self.central_thread.daemon = True
            self.central_thread.start()

            # Initialize Treadmill
            control_point_char.write_value(bytearray([0x00]), flags={})
            time.sleep(0.25)
            control_point_char.write_value(bytearray([0x01]), flags={})
            time.sleep(0.25)
            control_point_char.write_value(bytearray([0x00]), flags={})

            # Monitor loop
            while not self.stop_event.wait(0.1):
                if not monitor.connected:
                    print("Central Device disconnected")
                    break
                if self.ftms_control_value[0] is True:
                    print(self.ftms_control_value[2])
                    control_point_char.write_value(self.ftms_control_value[2], flags={})
                    self.ftms_control_value[0] = False
                    self.ftms_control_value[1] = True

        except Exception as e:
            print(f"connect_and_run error ({type(e).__name__}: {e})")

        finally:
            self.connected = False
            self.values = [0] * 10
            self._reset_adapter(dev_address)

    def on_new_fm_measurement(self, iface, changed_props, invalidated_props):

        test_value = changed_props.get('Value', None)
        if not test_value:
            return
        else:
            self.ftms_status_value = test_value

    def on_new_ts_measurement(self, iface, changed_props, invalidated_props):

        test_value = changed_props.get('Value', None)
        if not test_value:
            return
        else:
            self.training_status_value = test_value

    def on_new_ftms_measurement(self, iface, changed_props, invalidated_props):
        """
        Callback used to receive notification events from the device.
        Parses FTMS Treadmill Data (0x2ACD) with variable-length fields
        based on the flags bitmap per the FTMS spec.
        :param iface: dbus advanced data
        :param changed_props: updated properties for this event, contains Value
        :param invalidated_props: dbus advanced data
        """

        test_value = changed_props.get('Value', None)
        if not test_value:
            return
        else:
            self.value = test_value

        raw = bytes(self.value)
        if len(raw) < 2:
            return

        flags = struct.unpack_from('<H', raw, 0)[0]

        # Bit 0 = 0 means speed is present (FTMS treadmill data).
        # Other characteristics (e.g. 2ADA status, 2AD3 training status) also trigger this
        # callback via bluezero's global signal handler. Filter them out here.
        if flags & 0x0001:
            return

        offset = 2
        values = [0] * 10
        _log_this = not hasattr(self, '_dbg_count') or self._dbg_count % 40 == 0
        if not hasattr(self, '_dbg_count'):
            self._dbg_count = 0
        self._dbg_count += 1

        # Bit 0 = 0 means Instantaneous Speed IS present (inverted logic)
        if not (flags & (1 << 0)):
            if offset + 2 <= len(raw):
                values[0] = struct.unpack_from('<H', raw, offset)[0]
                offset += 2

        # Bit 1: Average Speed (uint16) — skip
        if flags & (1 << 1):
            offset += 2

        # Bit 2: Total Distance (uint24, 3 bytes little-endian)
        if flags & (1 << 2):
            if offset + 3 <= len(raw):
                b = raw[offset:offset + 3]
                values[1] = b[0] | (b[1] << 8) | (b[2] << 16)
                offset += 3

        # Bit 3: Inclination (sint16) + Ramp Angle Setting (sint16)
        if flags & (1 << 3):
            if offset + 4 <= len(raw):
                values[3], values[4] = struct.unpack_from('<hh', raw, offset)
                offset += 4

        # Bit 4: Positive Elevation Gain (uint16) + Negative Elevation Gain (uint16) — skip
        if flags & (1 << 4):
            offset += 4

        # Bit 5: Instantaneous Pace (uint8) — skip
        if flags & (1 << 5):
            offset += 1

        # Bit 6: Average Pace (uint8) — skip
        if flags & (1 << 6):
            offset += 1

        # Bit 7: Expended Energy — Total (uint16) + per Hour (uint16) + per Minute (uint8)
        if flags & (1 << 7):
            if offset + 5 <= len(raw):
                values[5] = struct.unpack_from('<H', raw, offset)[0]
                values[6] = struct.unpack_from('<H', raw, offset + 2)[0]
                values[7] = raw[offset + 4]
                offset += 5

        # Bit 8: Heart Rate (uint8)
        if flags & (1 << 8):
            if offset + 1 <= len(raw):
                values[8] = raw[offset]
                offset += 1

        # Bit 9: Metabolic Equivalent (uint8) — skip
        if flags & (1 << 9):
            offset += 1

        # Bit 10: Elapsed Time (uint16)
        if flags & (1 << 10):
            if offset + 2 <= len(raw):
                values[9] = struct.unpack_from('<H', raw, offset)[0]
                offset += 2

        self.values = values
        if _log_this:
            print(f"[FTMS] flags=0x{flags:04X} payload_len={len(raw)-2} id(self)={id(self)} id(self.values)={id(self.values)} values={values[:4]}")

    def scan_for_ftms(self):
        """Scan for BLE devices advertising the FTMS service UUID on our adapter."""
        print(self.adapter_address)
        dongle = self._get_dongle()
        dongle.nearby_discovery(timeout=5.0)

        managed = dbus_tools.get_managed_objects()

        # Find the D-Bus object path for our adapter by its MAC address.
        adapter_path = None
        for path, ifaces in managed.items():
            iface = ifaces.get(constants.ADAPTER_INTERFACE)
            if iface and str(iface.get('Address', '')).upper() == self.adapter_address.upper():
                adapter_path = path
                break

        if adapter_path is None:
            return

        for path, ifaces in managed.items():
            dev = ifaces.get(constants.DEVICE_INTERFACE)
            if dev is None:
                continue
            if not path.startswith(adapter_path + '/'):
                continue
            uuids = [str(u).lower() for u in dev.get('UUIDs', [])]
            if self.ftms_srv.lower() not in uuids:
                continue
            address = str(dev.get('Address', ''))
            if self.blacklist_address == address:
                continue
            print(address)
            yield types.SimpleNamespace(
                address=address,
                adapter=self.adapter_address,
                alias=str(dev.get('Alias', address)),
            )
