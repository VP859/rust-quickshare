// Checks the GUI frontend (src-tauri/ui) without a browser:
//  - every JavaScript module parses (`node --check`),
//  - inline <script> blocks are absent, so the CSP can forbid them,
//  - every translation key used by the page or the scripts exists in every
//    language in js/locales/, all languages define the same keys and no key
//    is written twice in one file,
//  - the copies of @noble/ciphers in src/web/page/vendor/ (the encryption of
//    browser sessions) are byte for byte those of the npm package.
//
// Usage: node scripts/check-frontend.mjs src-tauri/ui
// Linting and browser tests: `npm run lint:ui`, `npm run test:ui`.

import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { pathToFileURL } from 'node:url';

const dir = process.argv[2];
if (!dir) {
  console.error('usage: node scripts/check-frontend.mjs <ui directory>');
  process.exit(2);
}

/** All files below `root` whose name ends with `ext`. */
function filesIn(root, ext) {
  return readdirSync(root, { withFileTypes: true, recursive: true })
    .filter((e) => e.isFile() && e.name.endsWith(ext))
    .map((e) => join(e.parentPath, e.name));
}

let failed = false;
const scripts = filesIn(dir, '.js');
for (const file of scripts) {
  try {
    execFileSync(process.execPath, ['--check', file], { stdio: 'pipe' });
  } catch (e) {
    console.error(`syntax error in ${file}:\n${e.stderr}`);
    failed = true;
  }
}

const html = readFileSync(join(dir, 'index.html'), 'utf8');
if (/<script(?![^>]*\bsrc=)[^>]*>/.test(html)) {
  console.error('index.html contains an inline <script>; the CSP only allows script files');
  failed = true;
}

const languages = {};
for (const file of filesIn(join(dir, 'js', 'locales'), '.js')) {
  const { default: texts } = await import(pathToFileURL(file).href);
  languages[basename(file, '.js')] = new Set(Object.keys(texts));
  // A key written twice silently keeps only the last value.
  // String contents are blanked first, so a colon inside a text is no key.
  const source = readFileSync(file, 'utf8').replace(/'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\])*`/g, "''");
  const seen = new Set();
  for (const [, key] of source.matchAll(/(?:^|[\s,{])([a-z][a-z0-9_]*)\s*:/gm)) {
    if (seen.has(key)) {
      console.error(`${basename(file)}: key written twice: ${key}`);
      failed = true;
    }
    seen.add(key);
  }
}
const codes = Object.keys(languages).sort();
if (codes.length === 0) {
  console.error('no languages in js/locales');
  process.exit(1);
}

const code = scripts.map((f) => readFileSync(f, 'utf8')).join('\n');
const used = new Set([
  ...[...html.matchAll(/data-i18n(?:-ph|-aria|-title)?="(\w+)"/g)].map((m) => m[1]),
  ...[...code.matchAll(/\bt\('(\w+)'/g)].map((m) => m[1]),
]);
// Keys built dynamically: t('st_' + kind) and the like; every key with such a
// prefix must then exist in all languages, which the comparison below ensures.
for (const prefix of [...code.matchAll(/\bt\('(\w+_)' \+/g)].map((m) => m[1])) used.delete(prefix);

for (const lang of codes) {
  const missing = [...used].filter((k) => !languages[lang].has(k));
  if (missing.length) {
    console.error(`${lang}: missing keys: ${missing.join(', ')}`);
    failed = true;
  }
  for (const other of codes) {
    const extra = [...languages[lang]].filter((k) => !languages[other].has(k));
    if (extra.length) {
      console.error(`keys in ${lang} but not in ${other}: ${extra.join(', ')}`);
      failed = true;
    }
  }
}

if (failed) process.exit(1);
// The cipher of browser sessions is served from vendor/; it must be exactly
// the audited package that package-lock.json pins.
const root = join(import.meta.dirname, '..');
const vendor = join(root, 'src', 'web', 'page', 'vendor');
const npm = join(root, 'node_modules', '@noble', 'ciphers');
if (!existsSync(npm)) {
  console.error('node_modules/@noble/ciphers is missing: run `npm ci` to check src/web/page/vendor/');
  failed = true;
} else {
  for (const file of readdirSync(vendor).filter((f) => f !== 'README.md')) {
    if (!readFileSync(join(vendor, file)).equals(readFileSync(join(npm, file)))) {
      console.error(`src/web/page/vendor/${file} differs from @noble/ciphers ${JSON.parse(readFileSync(join(npm, 'package.json'))).version}`);
      failed = true;
    }
  }
}

if (failed) process.exit(1);
console.log(`ok: ${scripts.length} script(s), ${used.size} keys used, languages: ${codes.join(', ')}`);
