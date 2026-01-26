# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

RoCo is an embedded Rust application targeting Raspberry Pi (aarch64-unknown-linux-gnu). It interfaces with a u-blox ZED-F9P GNSS receiver via serial/UBX protocol and renders real-time GPS data to an 800x480 display using Linux DRM/KMS.

## Build Commands

All builds use `just` and cross-compile via WSL (Ubuntu) for aarch64 Linux:

```bash
just check          # Type-check without building
just build          # Debug build for aarch64
just build-release  # Release build for aarch64
just clean          # Clean build artifacts
just deploy         # Build, send to Pi, send data, chmod, and SSH into Pi
just send           # SCP binary to Pi (debug build)
just send-data      # SCP ./data directory to Pi
just ssh            # SSH into the Pi
just loc            # Count lines of code (tokei)
```

The Pi connection is configured in the `justfile` (REMOTE, REMOTE_PATH variables).

## Architecture

### Module Structure

- **`src/main.rs`** - Tokio single-threaded async runtime (`current_thread` flavor for power efficiency). Initializes display, GNSS, and debug terminal, then runs the main render loop.

- **`src/io/`** - I/O subsystems
  - `gps.rs` - GNSS task orchestration: `init_gnss_task()` spawns the UBX hub and periodic poll/aggregation tasks. Returns `GNSSHandles` containing a `GNSSFixHandle` (watch channel for latest fix) and `UBXHub` (for sending commands).
  - `ubx.rs` - Complete u-blox UBX binary protocol implementation. `UBXHub` is an async router that manages serial read/write with topic-based pub/sub for parsed messages. Contains `UBXPoll` enum for poll requests, `UBXData` enum for parsed responses, and typed structs in `nav::`, `mon::`, `sec::`, `cfg::` submodules.
  - `at.rs` - AT command utilities
  - `data.rs` - Data structures

- **`src/graphics/`** - Display subsystems
  - `display.rs` - Linux DRM/KMS double-buffered framebuffer using dumb buffers and XRGB8888 format. `KMSDisplay::new()` auto-detects the KMS-capable card and connector. Uses `AsyncFd` for non-blocking page flip waiting.
  - `text.rs` - `FontAtlas` for rasterized glyph caching with disk serialization. Fonts are embedded at compile time from `.compile_time_data/`.
  - `debug_terminal.rs` - `DebugTerminal` integrates with `tracing` to display log messages on-screen.
  - `geometry.rs`, `color.rs`, `fast_draw.rs` - Drawing primitives

### Key Patterns

**Async UBX Communication**: The `UBXHub` provides a clean async interface for the UBX protocol:
- `hub.topic(UBXPoll::NAVPVT).subscribe()` - Subscribe to a message type
- `hub.send_poll(UBXPoll::NAVPVT)` - Send a poll request
- `hub.request(frame, expected, timeout)` - Request-response pattern

**GNSS Fix Aggregation**: `GNSSFix` aggregates multiple UBX message types (NAV-PVT, NAV-DOP, NAV-SIG, etc.) into a single struct with Arc-wrapped fields for cheap cloning. Updated via `tokio::sync::watch`.

**Double Buffering**: `KMSDisplay` uses front/back Frame structs. Call `queue_flip()`, await `wait_for_pageflip()`, then `swap()` and `copy_front_to_back()`.

## Target Platform

- Target: Raspberry Pi (aarch64-unknown-linux-gnu)
- Display: 800x480 via HDMI/DRM
- GPS: u-blox ZED-F9P on `/dev/serial0` (UART, 38400→230400 baud)
- Cross-compilation requires WSL with `aarch64-linux-gnu-gcc` toolchain

## Tracing

All logging uses `tracing` macros with a `target:` to identify the source in the pipeline:

```rust
tracing::info!(target: "GNSSPoll", "NAVPVT poll send failed: {:?}", e);
tracing::debug!(target: "UBXCfg", "CFGVALSET layers: ram={}, bbr={}, flash={}", ram, bbr, flash);
```

Existing targets: `"InitGNSSTask"`, `"GNSSPoll"`, `"UBXHub"`, `"UBXHubReader"`, `"UBXHubWriter"`, `"UBXCfg"`, `"DebugTerminal"`, `"Main"`. The target string colors the `[tag]` in the on-screen debug terminal.

## Hardware Code Guidelines

This is embedded/hardware code—small mistakes in binary parsing cause hard-to-diagnose failures:

- **Double-check byte indices** against the protocol spec (UBX docs). Off-by-one errors are common.
- **Prefer error propagation** (`return Err(...)`) over silent failures (`_ => {}`).
- **Validate payload sizes** before indexing into buffers.
- **Use hex formatting** (`{:#04X}`, `{:02X}`) when logging protocol bytes.

## Commit Style

Use lowercase commit messages.

## Dependencies of Note

- `tokio` + `tokio-serial` - Async runtime and serial port
- `drm` + `drm-fourcc` - DRM/KMS display interface
- `memmap2` - Memory-mapped framebuffers
- `ab_glyph` - Font rendering
- `tracing` - Structured logging (integrated with on-screen debug terminal)
- `num_enum` - Primitive-to-enum conversion for UBX parsing
