# Changelog

## [Unreleased]

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
