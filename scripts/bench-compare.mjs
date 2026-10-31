// Compares Criterion results against a saved baseline and fails on a clear
// performance regression.
//
// Run the benchmarks of the base revision with `-- --save-baseline base`, then
// those of the change with `-- --baseline base`; Criterion then writes the
// relative change of every benchmark to `<criterion dir>/**/change/estimates.json`.
//
// Usage: node scripts/bench-compare.mjs <criterion dir> [threshold percent, default 15]
//
// A benchmark counts as a regression only when even the lower bound of the 95 %
// confidence interval of its mean time is above the threshold, so the noise of
// shared CI machines does not fail a build.

import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

const [dir, thresholdArg = '15'] = process.argv.slice(2);
if (!dir) {
  console.error('usage: node scripts/bench-compare.mjs <criterion dir> [threshold percent]');
  process.exit(2);
}
const threshold = Number(thresholdArg) / 100;

const changes = readdirSync(dir, { recursive: true })
  .map(String)
  .filter((p) => p.endsWith(join('change', 'estimates.json')));
if (changes.length === 0) {
  console.error(`no comparisons in ${dir}; was the baseline saved and compared?`);
  process.exit(1);
}

const pct = (x) => `${x >= 0 ? '+' : ''}${(x * 100).toFixed(1)} %`;
const rows = changes.map((file) => {
  const { mean } = JSON.parse(readFileSync(join(dir, file), 'utf8'));
  const name = relative('.', file.split(`${sep}change${sep}`)[0]);
  return { name, mean: mean.point_estimate, low: mean.confidence_interval.lower_bound };
});
rows.sort((a, b) => b.mean - a.mean);

console.log(`Time change against the base revision (regression: lower bound > ${pct(threshold)}):`);
for (const r of rows) {
  const flag = r.low > threshold ? '  REGRESSION' : '';
  console.log(`  ${pct(r.mean).padStart(9)}  (≥ ${pct(r.low)})  ${r.name}${flag}`);
}

const regressions = rows.filter((r) => r.low > threshold);
if (regressions.length) {
  console.error(`${regressions.length} benchmark(s) got slower by more than ${pct(threshold)}`);
  process.exit(1);
}
