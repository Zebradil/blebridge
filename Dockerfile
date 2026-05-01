FROM python:3.11-slim-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc \
    pkg-config \
    libglib2.0-dev \
    libgirepository1.0-dev \
    libcairo2-dev \
    libdbus-1-dev \
    libdbus-glib-1-dev \
    libusb-1.0-0-dev \
    bluez \
    && pip install --no-cache-dir \
    "PyGObject<3.50" \
    dbus-python \
    bluezero \
    openant \
    pyusb \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY blebridge.py antsend.py ble_central.py ble_peripheral.py ftms.py utils.py ./

CMD ["python", "-u", "blebridge.py"]
