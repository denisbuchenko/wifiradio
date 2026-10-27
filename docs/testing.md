# Verification

Run `powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Check` for formatting, Clippy with warnings denied, automated tests, and the release build.

The automated suite covers:

- First-call warning and the 179/180-second idle boundary.
- Replay rejection with out-of-order packets and counter jumps.
- Every wire message, bounded packet sizes, wrong-room rejection, authenticated tamper rejection, and truncated datagrams.
- Three softly faded tones, their amplitude bound, silence between tones, and exact 480 ms duration.
- Wi-Fi frame reordering, missing-frame silence, and rejection of late frames.
- Two independent engines using real loopback UDP sockets with synthetic input/output: discovery, warnings on both sides, capture starting only after the warning, bidirectional audio, no repeated warning in a warm conversation, immediate capture stop on release, and graceful disconnect.
- Repeated Start after a lost Ready, a receiver requesting a warning after restart, simultaneous presses, capture shutdown on peer timeout, and a delayed Start arriving after cancellation.
- A slow output device preventing Ready and capture until the actual warning playback finishes.
- Correct directed broadcasts from on-link prefixes, explicit outgoing interface selection, bounded unicast search, and excluding own/network/broadcast/VPN addresses from manual connection.
- Two engines discovering each other with all broadcast and ordinary Hello destinations removed, using only the fallback unicast probe path before exercising bidirectional voice.
- Manual peer mode ignoring a different authenticated radio.

An opt-in test checks the release GUI against a real authenticated UDP test peer. Start `dist/WifiRadio.exe --no-autostart`, then run `cargo test native_window_follows_real_peer_discovery_and_disconnect -- --ignored` with the project Rust environment. It verifies that the native button becomes enabled with the accessible label «Держать и говорить» after discovery and becomes disabled after the peer exits. Presence alone makes no sound. It does not press the button or access the microphone.

`cargo test native_window_reopens_after_close_on_a_second_launch -- --ignored` checks the actual release GUI: three cycles of closing with X, verifying that the window hides, launching the same executable again (including `--background`), and verifying that the same window returns and the duplicate exits. Run native GUI tests sequentially, with no active conversation.

The same loopback scenario is available in the built GUI executable:

```powershell
Start-Process -FilePath .\dist\WifiRadio.exe -ArgumentList '--self-test', '.local/self-test.txt' -Wait -WindowStyle Hidden
Get-Content .local/self-test.txt
```

It performs no microphone/speaker calls, autostart changes, or firewall changes. UI checks should use `--no-autostart` and exit the test app afterwards.

For acceptance on two real PCs:

1. Copy the same release executable to both PCs; allow it on the private network. Verify yellow appears on both sides.
2. Hold the button after initial discovery. Verify three tones on both speakers, no speech before the tones, and intelligible speech after «Говорите».
3. Release and reply from the other PC. Verify no extra warning during the active conversation.
4. Wait at least three minutes after the last voice frame. Verify that both sides warn again on the next press.
5. During a press, switch windows, close/minimize the app window, and release the mouse outside the button. In each case capture must stop.
6. Close the other app completely. Verify grey within five seconds. Reopen it and verify recovery.
7. Verify the app receives audio while hidden in the tray, survives an Explorer restart, and opens from the notification icon.
8. Verify startup after Windows sign-in and that disabling startup remains effective after relaunch.
9. Close with X, then launch the same executable again. Verify the existing window returns. Repeat with a shortcut containing `--background`.
10. With each PC's VPN enabled, verify presence and bidirectional speech. Test different VPN clients if used. If discovery fails, use Connection to enter the other PC's on-link Wi-Fi/Ethernet IP. Check that empty input restores automatic search and survives relaunch. A VPN LAN block still requires a user-configured exception.

Loopback verification cannot certify microphone permissions, sound quality, latency on the user's router, firewall settings, or the second PC. Record only checks actually performed.

## Checked on this computer

On 2026-10-27, the thirteen automated tests passed, the opt-in native GUI discovery/disconnect test passed, formatting and Clippy passed, and the release executable passed its own loopback self-test. The release was built with project-local Rust 1.99.1 for `x86_64-pc-windows-gnu` and was 393,216 bytes.

The native Russian window was inspected at 125% display scaling. Accessible controls exposed the offline push-to-talk button as disabled and the overflow menu as enabled. Closing the window kept the process alive; a notification callback reopened it; a second launch reused the window and exited. The `--no-autostart` launch did not create the startup marker. The first real network launch prompted Windows Firewall for permission; no firewall rule was changed by these checks.

After testing, the final executable was launched with `--background` for normal use. Current-user startup registration and its exact executable path were checked, and the first-launch marker was present. Windows Shell confirmed the notification icon exists through a no-fields `NIM_MODIFY` call. `Shell_NotifyIconGetRect` returned `E_FAIL` for the hidden icon on this computer, so it was not used as a presence assertion. The release SHA-256 matched the delivered checksum.

Sound through physical microphones and speakers on two different PCs, real router latency, Explorer restart, and sign-in startup still require the acceptance steps above.

## Version 0.2.0 follow-up

On 2026-10-27, formatting, Clippy with warnings denied, nineteen automated tests, the release build, and the executable's unicast-only synthetic audio self-test passed. The built executable was 427,520 bytes. The opt-in close/relaunch test passed all three cycles on the running release. It used `--no-autostart` and did not change startup registration.

Read-only adapter/route inspection found an active physical Ethernet LAN and an active Amnezia WireGuard adapter with a default route. The diagnostic executable selected Ethernet, calculated the correct directed broadcast, and successfully applied `IP_UNICAST_IF`. The old library broadcast was also correct on this particular PC: the potential prefix-selection bug was not established as the cause here.

The new release automatically discovered an authenticated peer on another LAN address while Amnezia remained active. The Connection window showed ongoing authenticated incoming packets, and the main window exposed «Держать и говорить». Local addresses and captured UI images stay in ignored `.local` reports. This verifies real LAN presence with this VPN configuration; it does not certify all VPN products or physical audio.

The opt-in synthetic-peer GUI discovery test could not run in isolation: its required offline precondition failed because the real peer was already connected. This was not counted as a passed test. Its earlier 0.1.0 result remains recorded above. No firewall, VPN, route, microphone, or speaker settings were changed. Actual spoken audio on the two physical PCs still needs user verification.
