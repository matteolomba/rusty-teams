# Changelog

## [Unreleased]

## [0.7.0-rc.7] - 2026-11-27

### Changed
- Synced the MSAL unseal helper with upstream and added the review fixes for Windows persistence: one success is enough, a closed tab or WebDriver BiDi does not count, and Retry also wakes the page-runner backoff.

## [0.7.0-rc.6] - 2026-11-27

### Changed
- Integrated upstream PRs #147 (quiet Wayland portal stop), #140 (quiet Linux audio-device dumps in diagnostics), #141 (Linux call tones on the saved Pulse sink) and #139 (macOS host-load report and early send limits).

### Fixed
- The code-block composer test no longer compares the internal `U+FFF9` header marker with visible text.

## [0.7.0-rc.5] - 2026-11-27

### Added
- Italian labels for calls, chat, settings, calendar, files, dialogs and notifications, so every screen follows the selected UI language.

### Changed
- Synced with `danieldaeschle/rusty-teams` up to `c5f9d259`; fork choices on updates, localization and the Windows release stay in place.
- CI checks `cargo fmt --check` on every pull request.

### Fixed
- Windows keeps the MSAL token-cache key while signed in, so a WebView2 restart no longer leaves the app online without tokens and reconnecting forever.
- Trouter disconnect reasons are written to the diagnostics log and the Reconnecting banner offers Retry.
- The MSAL cache key is persisted right after sign-in, so a crash or OOM kill no longer forces another sign-in.
- The stopped-screen-share notice and the Help yes/no values use translation keys that exist in all locales.

## [0.7.0-rc.4] - 2026-11-25

### Changed
- Synced with `danieldaeschle/rusty-teams` up to `797e287e`; fork choices on updates, localization and PR gate preserved.
- Message preview preference now uses the Teams key `simpleCollabUserSettings.hasUserEnabledMessagePreviews`, with a fallback to the legacy key.
- Workspace Clippy lints for undocumented `unsafe` blocks and attributes without a reason.

### Fixed
- Guest org with interactive MFA: `AADSTS50076` is no longer dropped, the splash no longer blocks org switching, and a Return to home org action is available.
- Browser shutdown bounds the MSAL cache-key save to 500 ms and validates cookie fields.
- Strict parsing for agent settings and sessions, web shadow events, and local files: unknown fields and decode errors are no longer silently ignored.
- Remote screen share: folded `applicationsharing-video` lines keep their slot, the indicator and route work without a subscription link, and an already-active share is routed when the call is answered.
- Initial video descriptions and bandwidth seed on group offers, aligned with Teams.
- Session rejections and answers without an SDP answer now end the call with an explicit state instead of being ignored.
- Active share line sent as `sendonly` and stop tag `;v_2`/`;v_1` chosen from the camera state.
- Direct renegotiation modes aligned with Teams: no `ScreenViewer` in the offer, `Video` in the answer when the video line is accepted.
- Realtime reconnect with backoff from 5 seconds to 5 minutes and jitter; the `/tpsUpdate/calendar` push updates the calendar.
- Chat list sends `startTime=0` and `targetType` on the first page, as in the web client.

## [0.7.0-rc.3] - 2026-11-25

### Added
- Local semgrep gate with pinned GitHub Actions and a documented security baseline.

### Fixed
- FFI, WebRTC and diagnostics hardening: no panic crosses native callbacks, logs and dumps are redacted and signal-safe, and retries and downloads have explicit limits.
- Disabled Windows self-swap, test-only IPC commands, silent SSO, Chromium flags that weakened protection, and unnecessary diagnostic features.


## [0.7.0-rc.2] - 2026-11-24

### Added
- Configurable update mode in Settings > General: `Off`, `Manual` (default), `Automatic`, `Release page`. The feed points to the fork and rejects upstream assets.

### Fixed
- The staged tag is compared with the released version, not the build id; calendar loading and colour indices were refactored without changing behaviour.

## [0.7.0-rc.1] - 2026-11-24

### Added
- "Show message preview" setting in the chat list: with preview off, rows become a single line.
- Denser Compact mode in conversations.
- Create meetings by dragging over empty calendar slots.
- People calendars from free/busy, with a dedicated settings group and custom colours.
- System idle detection on Linux through Wayland `ext-idle-notify-v1` with an X11 fallback.
- Native macOS picker for sharing a window (to test on macOS 14+).
- Italian UI translation, selectable in settings and applied automatically when the system language is Italian.

### Fixed
- VAAPI is no longer chosen as the default encoder; explicit selection remains available.
- Channel, team and tag mentions are rebuilt when a message is edited (to verify with a real Teams session).
- Handle `X_GetImage` `BadMatch` under X11 (to verify on real X11).
