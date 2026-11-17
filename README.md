# Herakles Mail

Herakles Mail is an offline-first, cross-platform JMAP desktop client built with React, TypeScript, Tauri, Rust, and redb. Stalwart is the initial server target. The interface and add-on model are based on [Bulwark Webmail](https://github.com/bulwarkmail/webmail), pinned as a Git submodule.

> **Development status:** This project is under active development. GitHub Actions builds packages for Windows, Linux, and macOS. Windows is the current development platform; full runtime validation on Linux/macOS, complete Bulwark feature parity, and production signing remain open.

## Downloads

Download the latest [release](https://github.com/herakles-now/herakles-mail-client/releases/latest):

| Platform | Download |
| --- | --- |
| Windows x64 | [Installer (.exe)](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/Herakles-Mail-windows-x64-setup.exe) |
| Linux x64 | [AppImage](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/Herakles-Mail-linux-x64.AppImage) · [Debian/Ubuntu (.deb)](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/Herakles-Mail-linux-x64.deb) |
| macOS Apple Silicon | [Disk image (.dmg)](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/Herakles-Mail-macos-arm64.dmg) |
| macOS Intel | [Disk image (.dmg)](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/Herakles-Mail-macos-x64.dmg) |

These development packages are unsigned; macOS uses ad-hoc signing without notarization. Your operating system may require approval to open them. Linux needs a desktop Secret Service keyring for credentials. Make the AppImage executable with `chmod +x Herakles-Mail-linux-x64.AppImage`.

[SHA256 checksums](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/SHA256SUMS) and a [complete source archive including Bulwark](https://github.com/herakles-now/herakles-mail-client/releases/latest/download/Herakles-Mail-source.tar.gz) accompany each release.

## Release builds

The [Desktop release workflow](https://github.com/herakles-now/herakles-mail-client/actions/workflows/release.yml) builds all four platform/architecture combinations. A manual run produces downloadable workflow artifacts without publishing a release. Pushing a tag such as `v0.1.0` publishes a release after every build succeeds; the tag must match the versions in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`. Keep both lockfiles current when changing versions. Tags containing a hyphen are marked as prereleases and do not replace the latest stable download links.

Packages, checksums, and complete sources are uploaded to a draft before publication. Existing published releases are never overwritten. GitHub's automatic source archives omit the submodule; use the complete source asset above instead.

## Architecture

The React interface reads and writes through Tauri commands. Rust owns credentials, the local redb database, and JMAP network access. Mail, calendar, and read-only contact/address-book data are cached locally for offline use; synchronization with Stalwart is explicit application code. Credentials are stored in the operating system keyring. Profile metadata and cached data are stored locally in redb.

Profiles are separate workspaces and can contain multiple accounts. Mail and calendar data are scoped by account. The app includes Bulwark's original interfaces with desktop adapters; the upstream submodule remains unchanged. Supported mail changes, structured sends, calendar event edits, and file metadata actions use durable native queues. Offline actions remain local until selected for transfer after reconnect; the separate desktop status bar shows and opens them. Its review view compares local intentions with the saved server baseline, supports exact local discard, and holds retried conflicts for a fresh approval. Uncertain sends cannot be retried there. Composer attachments and new file uploads are staged in redb and transferred by Rust as dependencies of approved writes. Uploads are limited to 25 MiB per file and 256 MiB of local staging per account. Contact editing and photos, file content replacement, cache cleanup, and full plug-in compatibility remain under development. Contact lists, details, shared address books, and search read from the local native replica, including offline. See [architecture notes](docs/architecture.md) and [add-on limitations](docs/addons.md).

The local redb database is currently unencrypted. Do not treat this development build as a production-secure client.

## Requirements

- Node.js 22 or later
- TypeScript 7 (the project intentionally requires TypeScript 7)
- Rust stable and Cargo, installed with [rustup](https://rustup.rs/)
- Platform prerequisites for [Tauri 2](https://v2.tauri.app/start/prerequisites/)

Install the Tauri system dependencies for your operating system before building. For Debian or Ubuntu Linux:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

On macOS, install Xcode Command Line Tools with `xcode-select --install`. On Windows, install Microsoft C++ Build Tools with **Desktop development with C++**; WebView2 is included with most current Windows installations. See Tauri's prerequisites page for distro-specific Linux packages and current platform details.

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
npm run audit:jmap            # Regenerate the Bulwark/Stalwart compatibility inventory
npm run check:jmap            # Reject stale inventories and unreviewed methods
npm run test:jmap-audit       # Test static extraction and review checks
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

`npm run dev` starts the browser interface; native JMAP, keyring, and redb operations require the Tauri app. `.env.example` documents debug-only test settings. Never commit `.env.local` or real credentials.

`npm ci` installs JavaScript dependencies only. When `npm run tauri dev` starts, the Tauri CLI invokes Cargo to compile the Rust desktop backend and then launches the app. No separate Rust build command is needed for development. Confirm the toolchain with `rustc --version` and `cargo --version`.

## Standalone Windows executable

Run `npm run tauri build -- --no-bundle` to build
`src-tauri/target/release/herakles-mail.exe`. Launch this executable directly:
it embeds the frontend and needs neither Node.js nor a Vite server at runtime.
The release build does not open a console window. Windows WebView2 is required.
Existing profiles, cached mail, and OS-keyring credentials remain in the same
application data locations. This produces an unsigned executable, not an installer.

## Desktop testing without automatic reloads

Build with `npm run tauri build -- --debug --no-bundle`, then launch
`src-tauri/target/debug/herakles-mail.exe` on Windows. This embeds the frontend
and does not use Vite or its automatic reloads. Use this build for interactive
mail tests while source files are being edited. `npm run tauri dev` deliberately
reloads or restarts after source/build-asset changes, triggering a startup sync.

## Acknowledgements

Herakles Mail owes a special thank-you to the [Bulwark Webmail](https://github.com/bulwarkmail/webmail) contributors. Bulwark's feature-rich React interface, mail and collaboration views, settings, and plug-in ecosystem are the foundation we are adapting for an offline-first desktop client. We preserve its upstream source, license, and attribution, and aim to share improvements that are useful upstream.

We are grateful to [Stalwart Labs](https://www.stalwart.email/) for building the open-source mail and collaboration server that is our primary JMAP interoperability target. Herakles connects to Stalwart through JMAP; Stalwart is a separate project, and this client does not bundle its server code or imply endorsement.

## Upstream and licensing

The Bulwark source is pinned in `upstream/bulwark.lock.json`; its own `LICENSE` and `NOTICE` remain in the submodule. Herakles Mail is licensed under **GNU AGPL version 3 only (AGPL-3.0-only)**; see [LICENSE](LICENSE) and [NOTICE](NOTICE) for project and upstream attribution. Third-party components retain their respective licenses.

## JMAP compatibility inventory

The [compatibility matrix](docs/jmap/matrix.md) maps Bulwark calls to pinned
Stalwart source locations, local implementations, limitations, and test evidence.
The accompanying JSON records every source occurrence, argument fields,
properties, filters, sort expressions, result references, dynamic methods,
and HTTP/push transports. It is a static audit, not a claim of full protocol
compatibility. See [audit maintenance](docs/jmap/README.md) for scope and updates.

For a read-only desktop inspection, set `HERAKLES_FORCE_READ_ONLY=1` in the
launching process. Any presence of this variable forces loaded accounts into
read-only mode without changing stored credentials or account settings. Remove
the variable entirely before a subsequent normal launch.
