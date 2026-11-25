# Changelog

## [Unreleased]

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
- Modalità aggiornamenti configurabile in Impostazioni > Generale: `Off`, `Manual` (predefinita), `Automatic`, `Release page`. Il feed punta al fork e rifiuta asset upstream.

### Fixed
- Confronto del tag staged con la versione rilasciata, non con il build id; caricamento calendario e indici colore refactorizzati senza cambiare comportamento.

## [0.7.0-rc.1] - 2026-11-24

### Added
- Impostazione "Mostra anteprima messaggi" nella lista chat: con l'anteprima disattivata le righe diventano a una sola linea.
- Modalità Compact più densa nelle conversazioni.
- Creazione meeting trascinando su slot vuoti nel calendario.
- Calendari delle persone da free/busy, con gruppo dedicato nelle impostazioni e colori personalizzabili.
- Rilevamento inattività di sistema su Linux tramite Wayland `ext-idle-notify-v1` e fallback X11.
- Picker nativo macOS per la condivisione di una finestra (da provare su macOS 14+).
- Traduzione italiana dell'interfaccia, selezionabile dalle impostazioni e applicata automaticamente quando la lingua del sistema è italiana.

### Fixed
- VAAPI non viene più scelto come encoder predefinito; la selezione esplicita resta disponibile.
- Ricostruzione delle mention canale/team/tag durante la modifica di un messaggio (da verificare con una sessione Teams reale).
- Gestione di `BadMatch` di `X_GetImage` sotto X11 (da verificare su X11 reale).
