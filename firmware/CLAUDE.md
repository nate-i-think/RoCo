# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

RoCo is an embedded Rust app for Raspberry Pi (aarch64). It interfaces with a u-blox ZED-F9P GNSS receiver via UBX protocol and renders to an 800x480 display via DRM/KMS.

The majority of the code was hand-written and validated. Be careful when making sweeping changes.

## Build

Cross-compiles via WSL. **Do not use `cargo` directly on Windows—use `just`:**

```bash
just check          # type-check
just build          # debug build
just build-release  # release build
just deploy         # build, scp to Pi, ssh in
```

## Reference Manuals

Protocol specs live in `./.manuals/`:
- `ubx protocol manual.pdf` - UBX binary protocol (source of truth for byte indices)
- `ZED-F9P_IntegrationManual_UBX.pdf` - GNSS receiver integration
- `quectel_*` - AT commands and hardware design for cellular modem

## Code Style

- **Lowercase commit messages**
- **Avoid `.expect()` and `.unwrap()`** - propagate errors with `Result`, don't panic
- **Prefer `parking_lot`** over `std::sync` for locks (no poisoning)
- **Use `tracing` with `target:`** to identify pipeline source:
  ```rust
  tracing::debug!(target: "UBXCfg", "parsing key: {:#010X}", key);
  ```
  Existing targets: `InitGNSSTask`, `GNSSPoll`, `UBXHub`, `UBXHubReader`, `UBXHubWriter`, `UBXCfg`, `DebugTerminal`, `Main`
- **Add periods at the end of comments**

## Hardware Code

Small mistakes in binary parsing cause hard-to-diagnose failures:
- **Double-check byte indices** against the protocol PDFs in `.manuals/`
- **Validate payload sizes** before indexing
- **Use hex formatting** (`{:#04X}`) when logging protocol bytes
- **Propagate errors** (`return Err(...)`) over silent `_ => {}`

## Architecture

- **`src/io/gps.rs`** - GNSS orchestration; `init_gnss_task()` returns `GNSSHandles`
- **`src/io/ubx.rs`** - UBX protocol parser and `UBXHub` async pub/sub router
- **`src/graphics/display.rs`** - DRM/KMS double-buffered framebuffer
- **`src/graphics/debug_terminal.rs`** - on-screen tracing display

## Working with Claude

When making edits, **explain rationale before the edit** so changes can be reviewed before approving.
