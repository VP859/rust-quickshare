# Translating Ferry

Ferry speaks English (default) and Polish. Translations live in three places:

| Part | File | Format |
|------|------|--------|
| Rust side: CLI, TUI, daemon, errors, notifications, tray | `locales/app.yml` | one entry per key with every language |
| GUI | `src-tauri/ui/js/locales/<code>.js` | one file per language |
| Android notification and share menu | `src-tauri/gen/android/app/src/main/res/values*/strings.xml` | Android string resources |

## Improving an existing translation

Edit the text, keep the placeholders (`%{name}` in `app.yml`, `{name}` in the GUI) and open a pull
request. Keep the tone short and friendly; "TICKET", "Node ID" and "Auto-Accept" are product terms
and stay untranslated.

## Adding a language

1. **`locales/app.yml`** – add a line for the new code to every entry:

   ```yaml
   pair.rejected:
     en: "The other device declined the pairing"
     pl: "Drugie urządzenie odrzuciło parowanie"
     de: "Das andere Gerät hat die Kopplung abgelehnt"
   ```

2. **`src/i18n.rs`** – add a variant to `Language`, to `Language::ALL`, `code()` and
   `native_name()`, and to the `every_key_is_translated_in_both_languages` test.
3. **GUI** – copy `src-tauri/ui/js/locales/en.js` to `<code>.js`, translate the values and add
   the language to `I18N` in `src-tauri/ui/js/i18n.js`.
4. **Android** – add `res/values-<code>/strings.xml` and the locale to `res/xml/locales_config.xml`.

Check the result:

```bash
cargo test i18n                                          # every key has every language
npm run check:ui                                         # every GUI key exists in every language
FERRY_LANG=<code> cargo run --bin ferry -- tui           # look at it
```

A hosted translation platform (Weblate, Crowdin) can be added later if there is interest; until
then, pull requests are the way to contribute.
