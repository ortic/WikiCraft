# Translating WikiCraft

The interface texts live in `web/src/locales/<code>.json`, one file per language. The English text
is the key, so a catalog is a plain map:

```json
{
  "Save": "Speichern",
  "Delete {name}?": "{name} löschen?",
  "{count} pages": { "one": "{count} Seite", "other": "{count} Seiten" }
}
```

- **Placeholders** in braces (`{name}`, `{date}`) are filled in by the app; keep them, in whatever
  order the language needs.
- **Plurals**: messages with `{count}` take an object with the language's plural categories
  (`zero`, `one`, `two`, `few`, `many`, `other`, as `Intl.PluralRules` names them; English and
  German need `one` and `other`, French `one`, `many` and `other`, Polish `one`, `few`, `many` and
  `other`).
- A text without a translation shows in English, so a catalog can be completed step by step.
- Search keywords (lower-case word lists such as `new add template`) work best with the English
  words kept after the translated ones.

The languages offered are listed in `web/src/i18n.ts` (`LANGUAGES`, with `rtl: true` for Arabic and
Hebrew, which turn the layout around) and on the server in `routes/branding.rs` (`LANGUAGES`).

## Workflow

```sh
cd web
node scripts/i18n.mjs extract     # src/locales/messages.json: every text the interface uses
node scripts/i18n.mjs check de    # what's missing in de.json, and translations that lost a placeholder
```

Code wraps interface text in `t()` (`import { t } from '../i18n'`): `t('Saved')`,
`t('Moved {title} to the trash', { title })`. Whole sentences make better translations than
pieces glued together, so pass names and numbers as placeholders instead of splitting a sentence
around them.

Not translated yet: e-mails, messages that come from the server (most error messages), and texts
inside exports (PDF/Word headings and footers).
