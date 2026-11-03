# Repository Guidelines

## Project Purpose & Architecture

Build Herakles Mail, a JMAP desktop client for Windows, Linux, and macOS with React, TypeScript ≥ 7, Tauri 2/Rust, and redb. Stalwart is the server; Ruby is not used.

Use Bulwark's original components and extension APIs. The pinned, unchanged submodule `upstream/bulwark` is the upstream foundation. Preserve LICENSE and NOTICE. Use explicit desktop adapters or checked build transforms instead of modifying the submodule or inventing a separate plugin format.

All UI data access must use redb through native IPC, including when online. Only the Rust synchronization worker may contact Stalwart. Keep credentials in Rust. redb supplies persistence; implement JMAP state tracking, deletions, retries, and conflicts explicitly. See `docs/architecture.md` and `docs/addons.md` for implemented behavior and limitations.

## Project Structure

- `src/desktop/`: original Bulwark host, routing, storage, and plugin adapters.
- `src/main.tsx`: application entry point.
- `src-tauri/src/`: native commands, local JMAP responder, synchronization, and redb storage.
- `scripts/prepare-bulwark.mjs`: reproducible sandbox and public-assets build.
- `tests/desktop/`: browser integration tests using synthetic data.
- Legacy `.env.local` / `.env.local.old`: ignored one-time migration source only; new credentials are stored through native Rust in the OS credential store.

## Development & Verification

Require Node.js ≥ 22, Rust/Cargo, and platform-specific Tauri prerequisites. TypeScript 7.0.2 is verified; do not downgrade. Initialize upstream with `git submodule update --init`, then run `npm ci`.

- `npm run tauri dev`: launch the desktop app.
- `npm run build`: prepare sandbox/assets, check TypeScript, and build the frontend.
- `npm run check:native`: check Rust compilation.
- `npm run test:native`: run Rust tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: check Rust formatting.
- `npm exec playwright -- test`: test the original UI and plugin lifecycle; start Vite preview on port 1430 first.

Keep npm/Cargo lockfiles current. Packaging and signing remain unconfigured.

## Style, Tests & Contributions

Use two-space TypeScript indentation, `rustfmt`, TypeScript `camelCase`, Rust `snake_case`, and `PascalCase` types. TypeScript strict checking is enabled. Test persistence, account isolation, state consistency, reconnects, and credential boundaries. No coverage threshold is established.

Use focused imperative commit subjects. Pull requests should describe behavior, verification, linked issues, configuration changes, and screenshots for interface changes.

## Security

Default to `JMAP_READ_ONLY=true`. The user has explicitly authorized `false` in this session for seen/flagged testing only. Never enable server writes without explicit user authorization. Live tests must force read-only transport and use temporary databases. Never print credentials or raw authenticated responses. Untrusted plugin frames must remain opaque-origin without native IPC access; privileged plugins remain unsupported.

The user explicitly authorized leaving read-only mode in this session. Writable testing currently covers only seen/flagged updates through the durable queue. The separately gated live_flag_write_roundtrip test is authorized to change one existing message flag and restore it; all other live tests remain read-only. Never print credentials or authenticated message contents.


