# syntax=docker/dockerfile:1.4
# ==========================================
# Stage 1: Builder
# ==========================================
FROM ghcr.io/astral-sh/uv:python3.11-bookworm-slim AS builder

# Cache apt lists and packages
RUN rm -f /etc/apt/apt.conf.d/docker-clean; echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

RUN --mount=type=cache,target=/var/lib/apt/lists \
    --mount=type=cache,target=/var/cache/apt,sharing=locked \
    apt-get update && apt-get install -y --no-install-recommends \
    gcc \
    pkg-config \
    libglib2.0-dev \
    libgirepository1.0-dev \
    libcairo2-dev \
    libdbus-1-dev \
    libdbus-glib-1-dev \
    libusb-1.0-0-dev

ENV UV_COMPILE_BYTECODE=1 UV_LINK_MODE=copy
WORKDIR /app

# Install dependencies first for better caching
RUN --mount=type=cache,target=/root/.cache/uv \
    --mount=type=bind,source=uv.lock,target=uv.lock \
    --mount=type=bind,source=pyproject.toml,target=pyproject.toml \
    uv sync --frozen --no-install-project --no-dev

# Install the project
ADD . /app
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev

# ==========================================
# Stage 2: Final Runtime Image
# ==========================================
FROM python:3.11-slim-bookworm

RUN rm -f /etc/apt/apt.conf.d/docker-clean; echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

RUN --mount=type=cache,target=/var/lib/apt/lists \
    --mount=type=cache,target=/var/cache/apt,sharing=locked \
    apt-get update && apt-get install -y --no-install-recommends \
    libglib2.0-0 \
    libgirepository-1.0-1 \
    gir1.2-glib-2.0 \
    libcairo2 \
    libdbus-1-3 \
    libdbus-glib-1-2 \
    libusb-1.0-0 \
    bluez

# Use --link to keep this layer independent of the base image and apt layers
COPY --link --from=builder /app /app
ENV PATH="/app/.venv/bin:$PATH"

WORKDIR /app

CMD ["python", "-u", "-m", "blebridge"]
