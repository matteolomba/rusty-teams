# Changelog

## [Unreleased]

### Added
- Impostazione "Mostra anteprima messaggi" nella lista chat: con l'anteprima disattivata le righe diventano a una sola linea.
- Modalità Compact più densa nelle conversazioni.
- Creazione meeting trascinando su slot vuoti nel calendario.
- Calendari delle persone da free/busy, con gruppo dedicato nelle impostazioni e colori personalizzabili.
- Rilevamento inattività di sistema su Linux tramite Wayland `ext-idle-notify-v1` e fallback X11.
- Picker nativo macOS per la condivisione di una finestra (da provare su macOS 14+).

### Fixed
- VAAPI non viene più scelto come encoder predefinito; la selezione esplicita resta disponibile.
- Ricostruzione delle mention canale/team/tag durante la modifica di un messaggio (da verificare con una sessione Teams reale).
- Gestione di `BadMatch` di `X_GetImage` sotto X11 (da verificare su X11 reale).
