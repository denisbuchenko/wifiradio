# Architecture

## Native desktop app

The application uses Rust and Win32 directly through `windows-sys` 0.59. There is no browser, WebView, server, installer, or external runtime. `ui.rs` owns the main window, two accessible owner-drawn native buttons, the notification icon, and a named per-session single-instance mutex. A `RefCell` prevents mutable aliasing when Windows reenters the window procedure. The UI polls an engine snapshot every 60 ms and sends Begin/End/Shutdown commands to its worker.

The push-to-talk button handles pointer capture, space key transitions, focus loss, and cancellation. Window close, minimize, and Escape hide the window. Only Exit stops the app. The tray icon is restored after an Explorer restart using `TaskbarCreated`.

`startup.rs` adds a current-user Run value on first real launch. A marker beside the executable prevents re-enabling startup after the user disables it. Tests use `--no-autostart` and never edit this value.

## LAN transport

`engine.rs` binds UDP 49735, sends an encrypted Hello once per second to the limited broadcast address and each adapter's directed broadcast address, and unicasts keepalives after discovery. Adapter addresses refresh every ten seconds so Wi-Fi changes recover without restarting. IP TTL is one. A peer expires after five seconds. Bye removes it immediately on a graceful exit. The first authenticated peer is selected; this version is designed for two PCs.

`protocol.rs` uses XChaCha20-Poly1305 with a random 256-bit build key. `build.rs` creates the ignored `.local/room.key` once and copies it to `OUT_DIR` for embedding. A random 128-bit process identity plus a monotonically increasing 64-bit packet counter form a unique 192-bit nonce. The protocol version `WR01` is authenticated additional data. Packet lengths and message types are validated before use. The selected peer has a 128-packet replay window, allowing bounded UDP reordering and rejecting duplicates. Restarting produces a new identity.

Messages: Hello, Bye, Start(talk id, warning flag), Ready(talk id, warning flag), Audio(talk id, frame sequence, PCM), End(talk id). Start retries every 100 ms for up to four seconds. Receivers acknowledge repeated starts without repeating the warning. End is sent three times. Stale utterance starts and audio are ignored. If both users press together, the lexicographically smaller node identity wins. Both sides support half-duplex audio.

## Audio and warning

`audio.rs` wraps the built-in WinMM waveform API. Audio is mono 16 kHz, signed 16-bit PCM, in 20 ms/640-byte frames. An audio datagram is 697 bytes including protocol fields, nonce, and authentication tag. Raw PCM is approximately 256 kbit/s before network overhead. This is suitable for a home LAN and needs no codec DLL.

The capture handle is created on press but recording starts only after the handshake. It is reset, unprepared, and closed on release. Boxed, explicitly aligned driver headers and boxed PCM buffers stay at stable addresses until Windows returns them. The speaker uses a bounded queue. Warning gates wait for both the nominal interval and the actual driver buffer completion, so a slow output device does not cause early recording. A 60 ms receive jitter buffer reorders frames, fills lost frames with silence, discards late frames, and bounds catch-up latency. No audio goes to disk.

`Activity` tracks actual outgoing and incoming frames rather than presence or button taps. The first conversation and any conversation after 180 seconds without frames trigger three 110 ms sine tones, each followed by 50 ms silence: C5/E5/G5, with short fades and 12% sample amplitude. The total is 480 ms. The sender plays its tones immediately; the receiver plays its own tones when Start arrives, then sends Ready after the warning interval. The sender starts capture only when it has Ready and its own interval has elapsed. A restarted receiver can request a warning through Ready, in which case the sender also warns before capture. A lost handshake fails closed.

## Scope

Windows 10/11 x64, two PCs on the same IPv4 LAN, default Windows input/output devices. No NAT traversal, recording, global hotkey, conferencing, or device selector. Network isolation on a guest SSID and firewall policy can prevent LAN discovery. Hardware sound and real Wi-Fi delivery require checks on the target PCs in addition to loopback tests.
