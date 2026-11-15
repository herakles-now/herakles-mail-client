Development release of Herakles Mail, an offline-first JMAP desktop client.

Downloads: Windows x64 installer, Linux x64 AppImage/DEB, macOS Apple Silicon and Intel DMG. Linux requires a desktop Secret Service keyring. AppImages must be made executable (`chmod +x`).

These builds are unsigned (macOS uses ad-hoc signing and is not notarized). Operating systems may require approval before opening. Cross-platform packaging does not establish full runtime or production readiness. Cached local mail is currently unencrypted.

Verify downloads against SHA256SUMS. The source archive includes the pinned Bulwark submodule; GitHub's automatic source archives do not.

See the README and architecture documentation for current functionality and limitations.
