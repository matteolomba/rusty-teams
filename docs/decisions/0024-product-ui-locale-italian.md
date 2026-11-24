---
status: accepted
date: 2026-11-24
---

# ADR 0024: Product UI also supports Italian

## Context

ADR 0008 shipped English and German for the app chrome. Italian-speaking users had to run the UI in a language they did not choose. The locale catalog already has one entry per language, and the settings screen already offers a language choice.

Options were: keep two languages, add Italian as a third locale, or let translators provide external catalogs. External catalogs would need a loader, a validation step and a distribution path for a set of strings that is small enough to ship in the binary.

## Decision

Product UI strings exist in English, German and Italian. First launch follows the OS locale when it is one of the three, otherwise English. The user can switch in the app, the choice is stored, and it applies without a restart. A missing translation still falls back to English.

Message bodies, names and other people's content stay untouched. New chrome strings go into the locale catalog with an English entry; German and Italian entries are expected.

## Consequences

- ADR 0008 is superseded by this record.
- The locale catalog is larger and every new string needs three translations before it is complete.
- Teams account language and app chrome language can still differ.
