// Not a hook: the skills (/qa, spec-build, spec-check) run the gates through
// this script instead of command by command. It lies under hooks/ next to
// the shared library.
//
//   node .claude/hooks/gates.mjs full|fast
//
// Runs every gate of the list regardless of `paths` (that filter is only for
// the Stop hook). Gates without `needs` between them run at the same time.
// The report on stdout is a table with the path of each gate's complete
// output plus, for every red gate, its error lines and its fix command.
// Exit 0 when everything is green, 1 otherwise, 2 without a usable gates.json.

import { createHash } from 'node:crypto';
import { mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { errorLines, loadGates, runGates } from './lib.mjs';

const LISTS = ['fast', 'full'];
const GATE_TIMEOUT_MS = 30 * 60 * 1000;
const MAX_ERROR_LINES = 60;

// .claude/hooks/ -> repository root, wherever the script is called from
const dir = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

function seconds(durationMs) {
  return `${(durationMs / 1000).toFixed(1)} s`;
}

function verdict(result) {
  switch (result.status) {
    case 'ok':
      return 'green';
    case 'failed':
      return 'red';
    case 'timeout':
      return `time limit (${GATE_TIMEOUT_MS / 60000} min)`;
    default:
      return result.blockedBy ? `skipped (${result.blockedBy} not green)` : 'skipped (unknown dependency)';
  }
}

function cell(text) {
  return text.replaceAll('|', '\\|');
}

const list = process.argv[2] ?? 'full';
const logFolder = join(tmpdir(), 'claude-gates', 'logs');
const logPrefix = `${createHash('sha256').update(dir).digest('hex').slice(0, 12)}-${list}-`;

/** Logs of the previous run: a gate skipped now must not point to its old output. */
function removeOldLogs() {
  try {
    for (const name of readdirSync(logFolder).filter(candidate => candidate.startsWith(logPrefix))) rmSync(join(logFolder, name), { force: true });
  } catch {
    // no logs yet
  }
}

/** The complete output of a gate; spec-check looks up single tests there, also in a green run. */
function writeLog(result, index) {
  mkdirSync(logFolder, { recursive: true });
  const file = join(logFolder, `${logPrefix}${index}-${result.gate.id}.log`.replace(/[^\w.-]/g, '_'));
  writeFileSync(file, result.output);
  return file;
}

if (!LISTS.includes(list)) {
  console.error(`Unknown list "${list}" (allowed: ${LISTS.join(', ')}).`);
  process.exit(2);
}

const gates = loadGates(dir)?.[list];
if (!Array.isArray(gates)) {
  console.error('.claude/gates.json is missing or invalid.');
  process.exit(2);
}

removeOldLogs();
const results = await runGates(dir, gates, {
  timeoutMs: GATE_TIMEOUT_MS,
  onSettled: result => console.error(`${verdict(result)}: ${result.gate.name} (${seconds(result.durationMs)})`),
});
const logs = results.map((result, index) => (result.status === 'skipped' ? null : writeLog(result, index)));

const report = ['| Gate | Command | Result | Duration | Log |', '| ---- | ------- | ------ | -------- | --- |'];
for (const [index, result] of results.entries()) {
  const duration = result.status === 'skipped' ? '-' : seconds(result.durationMs);
  report.push(`| ${cell(result.gate.name)} | \`${cell(result.gate.command)}\` | ${verdict(result)} | ${duration} | ${logs[index] ?? '-'} |`);
}

for (const [index, result] of results.entries()) {
  if (result.status !== 'failed' && result.status !== 'timeout') continue;
  report.push('', `### ${result.gate.name}: ${verdict(result)}`, '', `Command: \`${result.gate.command}\` in \`${result.gate.cwd ?? '.'}\``);
  if (result.gate.fix) report.push(`Auto-fixable with: \`${result.gate.fix}\``);
  report.push(`Full log: ${logs[index]}`, '', '```text', ...errorLines(result.output, MAX_ERROR_LINES), '```');
}

console.log(report.join('\n'));
process.exit(results.every(result => result.status === 'ok') ? 0 : 1);
