## Installing

| OS | File | How |
|---|---|---|
| Windows 10/11 (x64) | `…-setup.exe` | Run it. Installs for the current user; no administrator rights needed. |
| macOS 11+ (Apple silicon) | `….dmg` | Open it and drag Gravekeeper to Applications. |
| Debian, Ubuntu (x64 / arm64) | `…_amd64.deb` / `…_arm64.deb` | `sudo apt install ./<file>.deb` |
| Other Linux (x64 / arm64) | `…_x86_64.AppImage` / `…_aarch64.AppImage` | `chmod +x <file>.AppImage`, then run it. |

The packages are not code-signed yet, so the first start shows a warning:

- **Windows**: "Windows protected your PC" → *More info* → *Run anyway*.
- **macOS**: if it "cannot be opened", allow it under *System Settings → Privacy & Security → Open Anyway*, or run `xattr -dr com.apple.quarantine /Applications/Gravekeeper.app`.

To check a download: `sha256sum -c SHA256SUMS --ignore-missing`.
