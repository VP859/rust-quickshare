// Checks the GUI frontend (src-tauri/ui/index.html) without a browser:
//  - the inline <script> parses as JavaScript,
//  - every translation key used in the page exists in every language of the
//    I18N table, and all languages define the same keys.
//
// Usage: node scripts/check-frontend.mjs src-tauri/ui/index.html

import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const file = process.argv[2];
if (!file) {
  console.error('usage: node scripts/check-frontend.mjs <index.html>');
  process.exit(2);
}
const html = readFileSync(file, 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
if (scripts.length === 0) {
  console.error('no inline <script> found');
  process.exit(1);
}

let failed = false;
for (const [i, code] of scripts.entries()) {
  try {
    new vm.Script(code, { filename: `${file}#script${i}` });
  } catch (e) {
    console.error(`syntax error: ${e.message}`);
    failed = true;
  }
}

const script = scripts.join('\n');
const table = script.match(/const I18N = \{([\s\S]*?)\n  \};/);
if (!table) {
  console.error('I18N table not found');
  process.exit(1);
}
const languages = {};
for (const m of table[1].matchAll(/\n {4}(\w+): \{([\s\S]*?)\n {4}\},/g)) {
  languages[m[1]] = new Set([...m[2].matchAll(/(?:^\s+|, )(\w+): /gm)].map((k) => k[1]));
}
const codes = Object.keys(languages);
if (codes.length === 0) {
  console.error('no languages in the I18N table');
  process.exit(1);
}

const used = new Set([
  ...[...html.matchAll(/data-i18n(?:-ph|-aria|-title)?="(\w+)"/g)].map((m) => m[1]),
  ...[...script.matchAll(/\bt\('(\w+)'/g)].map((m) => m[1]),
]);
// Keys built dynamically: t('st_' + kind) and the like.
for (const prefix of [...script.matchAll(/\bt\('(\w+_)' \+/g)].map((m) => m[1])) used.delete(prefix);

for (const code of codes) {
  const missing = [...used].filter((k) => !languages[code].has(k));
  if (missing.length) {
    console.error(`${code}: missing keys: ${missing.join(', ')}`);
    failed = true;
  }
  for (const other of codes) {
    const extra = [...languages[code]].filter((k) => !languages[other].has(k));
    if (extra.length) {
      console.error(`keys in ${code} but not in ${other}: ${extra.join(', ')}`);
      failed = true;
    }
  }
}

if (failed) process.exit(1);
console.log(`ok: ${scripts.length} script(s), ${used.size} keys used, languages: ${codes.join(', ')}`);
