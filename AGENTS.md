# WifiRadio

Native Rust application for two Windows PCs on one LAN. The Russian UI is called «Рядом».

- Keep one push-to-talk control and the small overflow menu. Closing the window hides it in the notification area.
- Capture audio only while the control is held, after the warning handshake. Release on focus loss, mouse capture loss, hiding, and shutdown.
- After 180 seconds without transmitted or received audio, play C5/E5/G5 on both PCs before enabling capture. Warn on the first call too.
- The same executable goes to both PCs. `.local/room.key` is generated locally and embedded at build time; preserve it across rebuilds and never commit or print it.
- Keep build tools, caches, reports, and previews inside this project. Do not enable startup or change firewall rules during automated tests.
- Build: `powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Check`. Local Rust bootstrap: `scripts/bootstrap-rust.ps1`.
- Exit the running release app through its menu before rebuilding: Windows locks `dist/WifiRadio.exe` while it runs. Only stop this project's app.
- The project uses `windows-sys` 0.59, whose prebuilt import libraries allow builds with Rust's bundled GNU linker and no system C++ installation.

Details: [architecture](docs/architecture.md), [testing](docs/testing.md), [user instructions](README.md).
