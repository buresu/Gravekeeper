# Distribution

How Gravekeeper is packaged, installed, run as a service, signed and (in the future) updated.

## Packages

Installers are built with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) from the configuration in `crates/gravekeeper/Cargo.toml` (`[package.metadata.packager]`). Output goes to `target/packages/`.

| OS | Format | Installs to | Notes |
|---|---|---|---|
| Linux | `.deb` | `/usr/bin/gravekeeper`, menu entry, icons | Debian, Ubuntu and derivatives; amd64 and arm64 |
| Linux | `.AppImage` | runs from the file | Any distribution, x86_64 and aarch64; no menu entry unless integrated by the user's AppImage tool |
| Linux | `packaging/linux/install.sh` | `~/.local/bin`, `~/.local/share` | No package manager needed; `--uninstall` removes it |
| Windows | NSIS `…-setup.exe` | per user (no administrator rights) | Start menu entry, uninstaller |
| macOS | `.dmg` with `Gravekeeper.app` | `/Applications` (drag and drop) | Native architecture of the build machine |

Build locally (on the target OS):

```sh
cargo install cargo-packager --locked --version 0.11.8
cargo packager -p gravekeeper --release --formats deb,appimage   # Linux
cargo packager -p gravekeeper --release --formats nsis           # Windows
cargo packager -p gravekeeper --release --formats dmg            # macOS
```

`.github/workflows/release.yml` builds all of them on GitHub Actions; the Linux packages are built twice, on x64 and on arm64 runners (`ubuntu-22.04-arm`), because AppImages cannot be cross-packaged. Pushing a tag `vX.Y.Z` creates a **draft** release with the packages and a `SHA256SUMS` file; it is published by hand after a check.

On Windows the C runtime is linked statically (`.cargo/config.toml`), so the executable runs without the Visual C++ Redistributable. macOS builds target macOS 11 (`MACOSX_DEPLOYMENT_TARGET` in the workflow, `minimum-system-version` in `Cargo.toml`).

### Making a release

1. Set the new version in the root `Cargo.toml` (`[workspace.package] version`) and run `cargo check` so `Cargo.lock` follows.
2. Commit, then tag and push:

   ```sh
   git tag v0.2.0
   git push origin main v0.2.0
   ```

3. The release workflow checks that the tag matches the version (a mismatch fails the run), runs the whole CI, builds the packages on each OS and creates a draft release. Its text is `.github/release-notes.md` (installation notes) followed by the generated list of changes. A tag with a suffix such as `v0.2.0-rc.1` is marked as a pre-release.
4. Download and try the packages from the draft, then publish it on GitHub.

If the run fails, fix the problem, move the tag to the fixed commit (`git tag -f v0.2.0 && git push -f origin v0.2.0`) as long as the release is still a draft, and delete the old draft if one was created. Running the workflow by hand (*Actions → Release → Run workflow*) builds the packages as artifacts without making a release.

### Icons and desktop integration

The icon is drawn in code (`crates/gravekeeper/src/icon.rs`) and used directly for the window and tray. Packaged icon files are generated from it and committed:

```sh
cargo run -p gravekeeper --example gen_icons   # writes packaging/icons/*.png, .ico, .icns
```

- **Linux**: the desktop entry is `gravekeeper.desktop` with `StartupWMClass=gravekeeper`, matching the Wayland app id, so the window and task bar show the icon once the entry is installed (`.deb` or `install.sh`).
- **Windows**: `build.rs` embeds `gravekeeper.ico` in the executable when building on Windows.
- **macOS**: `gravekeeper.icns` is used for the app bundle.

## Running as a service

`gravekeeper --headless` runs the supervisor without a window. `gravekeeper service install` registers it per user:

| OS | Mechanism | Notes |
|---|---|---|
| Linux | systemd user unit `~/.config/systemd/user/gravekeeper.service` | Runs while the user is logged in; with `loginctl enable-linger <user>` it starts at boot and survives logout. `KillMode=mixed` lets Gravekeeper stop its applications before systemd intervenes. |
| macOS | LaunchAgent `~/Library/LaunchAgents/com.gravekeeper.supervisor.plist` | Started at login and restarted by launchd if it fails. |
| Windows | not yet | A Windows service needs a service control handler and runs in session 0 (no desktop); planned with the separate supervisor process. Use "Launch at login" instead. |

`gravekeeper service uninstall` stops and removes it; `gravekeeper service status` reports it.

**One supervisor at a time.** The service and the GUI use the same lock. While the service runs, opening the GUI shows "already running"; until the GUI can attach to a running supervisor (Phase 4, local IPC), choose one of:

- the GUI with "Launch at login" (desktop use), or
- the headless service, configured by editing `config.toml` and reloading it without stopping anything: `systemctl --user reload gravekeeper` on Linux, `launchctl kill SIGHUP gui/$(id -u)/com.gravekeeper.supervisor` on macOS (servers, machines without a desktop session).

## Code signing

Nothing is signed yet; the release workflow is prepared to sign when the secrets exist.

- **Windows (Authenticode)**: unsigned installers trigger SmartScreen warnings. Options: an OV/EV certificate in the runner's certificate store with `[package.metadata.packager.windows] certificate-thumbprint`, or `sign-command` for a cloud signing service (e.g. Azure Trusted Signing), which avoids handling a certificate file.
- **macOS (Developer ID + notarization)**: required for Gatekeeper to open the app without warnings. cargo-packager signs with `APPLE_SIGNING_IDENTITY` (certificate imported from `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD`) and notarizes with `APPLE_ID`, `APPLE_PASSWORD` (app-specific) and `APPLE_TEAM_ID`.
- **Linux**: packages are not signed individually; `SHA256SUMS` is published with each release. If an APT repository is offered later, it is signed with a repository key.

Signing keys are only ever stored as CI secrets, never in the repository.

## Updates (design)

Not implemented. The planned approach, in two steps:

1. **Notify.** Gravekeeper checks the latest GitHub release (at most once a day, can be turned off) and shows "version X is available" with a link. No download, no elevated rights, nothing installed automatically. Linux package-manager installs are left to the package manager.
2. **Install in place** (Windows, macOS, AppImage). Use `cargo-packager-updater`: the release workflow signs the packages with a minisign key (`cargo packager signer`), publishes an update manifest, and the app verifies the signature before replacing itself. Before installing, Gravekeeper stops its applications as it does on quit, and the user confirms.

Open questions before implementing: whether releases stay on GitHub (`buresu/Gravekeeper`), and whether updates should ever be automatic while applications are being supervised (proposal: never; always ask).
