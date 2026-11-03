# Herakles Mail

Herakles Mail is an offline-first, cross-platform JMAP desktop client built with React, TypeScript, Tauri, Rust, and redb. Stalwart is the initial server target. The interface and add-on model are based on [Bulwark Webmail](https://github.com/bulwarkmail/webmail), pinned as a Git submodule.

> **Development status:** This project is under active development. Windows is the current development platform; Linux and macOS builds, complete Bulwark feature parity, and production packaging have not yet been validated.

## Architecture

The React interface reads and writes through Tauri commands. Rust owns credentials, the local redb database, and JMAP network access. Mail and calendar data are cached locally for offline use; synchronization with Stalwart is explicit application code. Credentials are stored in the operating system keyring. Profile metadata and cached data are stored locally in redb.

Profiles are separate workspaces and can contain multiple accounts. Mail and calendar data are scoped by account. The app includes Bulwark's mail, settings, calendar, contacts, files, and plug-in interfaces, but not all of their server adapters are complete. Mail flag changes have a durable sync queue; calendar access is currently read-only. Composer, draft, send, contacts, files, and full plug-in compatibility remain under development. See [architecture notes](docs/architecture.md) and [add-on limitations](docs/addons.md).

The local redb database is currently unencrypted. Do not treat this development build as a production-secure client.

## Requirements

- Node.js 22 or later
- TypeScript 7 (the project intentionally requires TypeScript 7)
- Rust and Cargo
- Platform prerequisites for [Tauri 2](https://v2.tauri.app/start/prerequisites/)

## Build and run

```sh
git clone --recurse-submodules https://github.com/herakles-now/herakles-mail-client.git
cd herakles-mail-client
npm ci
npm run tauri dev
```

Useful checks:

```sh
npm run build                 # Type-check and build the frontend
npm run check:native          # Check the Rust/Tauri backend
npm run test:native           # Run Rust unit tests
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

`npm run dev` starts the browser interface; native JMAP, keyring, and redb operations require the Tauri app. `.env.example` documents debug-only test settings. Never commit `.env.local` or real credentials.

## Upstream and licensing

The Bulwark source is pinned in `upstream/bulwark.lock.json`; its own `LICENSE` and `NOTICE` remain in the submodule. Herakles Mail is licensed under **GNU AGPL version 3 only (AGPL-3.0-only)**; see [LICENSE](LICENSE) and [NOTICE](NOTICE) for project and upstream attribution. Third-party components retain their respective licenses.
