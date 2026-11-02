// Shared by the project hooks and the gate scripts. Plain Node without
// dependencies, so the hooks run in every clone. `.mjs` because a `.js` file
// would be ESM or CommonJS depending on the nearest package.json.

import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Claude Code writes one JSON document to stdin. On Windows the stream's 'end'
// does not reliably follow, so parse on every chunk and settle as soon as the
// document is complete; the idle timeout is only the fallback.
const STDIN_IDLE_TIMEOUT_MS = 1000;

export function readStdinJson() {
  return new Promise(resolve => {
    let data = '';
    let settled = false;
    let timer = null;

    const settle = value => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      try {
        process.stdin.destroy();
      } catch {
        // ignore
      }
      resolve(value);
    };
    const tryParse = () => {
      try {
        return JSON.parse(data);
      } catch {
        return undefined;
      }
    };
    const arm = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => settle(tryParse() ?? {}), STDIN_IDLE_TIMEOUT_MS);
    };

    process.stdin.setEncoding('utf8');
    process.stdin.on('data', chunk => {
      data += chunk;
      arm();
      if (!/\}\s*$/.test(data)) return;
      const parsed = tryParse();
      if (parsed !== undefined) settle(parsed);
    });
    process.stdin.on('end', () => settle(tryParse() ?? {}));
    process.stdin.on('error', () => settle({}));
    arm();
  });
}

export function projectDir(input) {
  return process.env.CLAUDE_PROJECT_DIR || input.cwd || process.cwd();
}

export function loadGates(dir) {
  try {
    return JSON.parse(readFileSync(join(dir, '.claude', 'gates.json'), 'utf8'));
  } catch {
    return null;
  }
}

const MAX_GATE_OUTPUT = 32 * 1024 * 1024;
// "nicht gefunden": cmd.exe reports a missing command in the system language
const ERROR_LINE = /error|×|✖|failed|FAIL|panicked|not found|not recognized|nicht gefunden/i;

/** A gate that runs past its time limit is killed with everything it started. */
function killTree(child) {
  try {
    if (process.platform === 'win32') spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true });
    else process.kill(-child.pid, 'SIGKILL');
  } catch {
    // already gone
  }
}

function runGate(dir, gate, timeoutMs) {
  return new Promise(resolve => {
    const started = Date.now();
    let output = '';
    let timedOut = false;
    let settled = false;
    const settle = status => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ status: timedOut ? 'timeout' : status, output, durationMs: Date.now() - started });
    };

    // Detached only outside Windows, for the process group the timeout kills:
    // there a detached process gets no console, and its shell would open a
    // visible window for every console program it starts
    const child = spawn(gate.command, {
      cwd: join(dir, gate.cwd ?? '.'),
      shell: true,
      windowsHide: true,
      detached: process.platform !== 'win32',
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, FORCE_COLOR: '0', NO_COLOR: '1', CARGO_TERM_COLOR: 'never' },
    });
    const collect = chunk => {
      if (output.length < MAX_GATE_OUTPUT) output += chunk;
    };
    child.stdout.setEncoding('utf8').on('data', collect);
    child.stderr.setEncoding('utf8').on('data', collect);
    const timer = setTimeout(() => {
      timedOut = true;
      killTree(child);
    }, timeoutMs);
    child.on('error', error => {
      output += `\n${error.message}`;
      settle('failed');
    });
    child.on('close', code => settle(code === 0 ? 'ok' : 'failed'));
  });
}

/** The gates each gate waits for, as indices. An unknown id is -1 and can never be met. */
function dependencies(gates) {
  const indexById = new Map(gates.map((gate, index) => [gate.id, index]));
  return gates.map(gate => (gate.needs ?? []).map(id => indexById.get(id) ?? -1));
}

/**
 * Runs a list from gates.json: every gate as soon as the gates it needs are
 * green, so independent gates run at the same time. A gate whose needs are
 * not green (or can never be met) is skipped. Results come back in list
 * order, each `{ gate, status, output, durationMs, blockedBy? }` with status
 * ok | failed | timeout | skipped; `onSettled` sees each one as it finishes.
 */
export function runGates(dir, gates, { timeoutMs, onSettled = () => {} }) {
  const needs = dependencies(gates);
  const results = gates.map(() => undefined);
  let running = 0;

  return new Promise(resolve => {
    const finish = (index, result) => {
      results[index] = { gate: gates[index], ...result };
      onSettled(results[index]);
    };

    const schedule = () => {
      let changed = true;
      while (changed) {
        changed = false;
        for (const [index, gate] of gates.entries()) {
          if (results[index]) continue;
          const blocking = needs[index].find(need => need === -1 || (results[need] && !['ok', 'running'].includes(results[need].status)));
          if (blocking !== undefined) {
            finish(index, { status: 'skipped', output: '', durationMs: 0, blockedBy: blocking === -1 ? null : gates[blocking].name });
            changed = true;
          } else if (needs[index].every(need => results[need]?.status === 'ok')) {
            results[index] = { gate, status: 'running' };
            running++;
            runGate(dir, gate, timeoutMs).then(result => {
              running--;
              finish(index, result);
              schedule();
            });
          }
        }
      }
      if (running > 0) return;
      // Whatever is still waiting now waits for itself: a cycle
      for (const index of gates.keys()) {
        if (!results[index]) finish(index, { status: 'skipped', output: '', durationMs: 0, blockedBy: null });
      }
      resolve(results);
    };

    schedule();
  });
}

/** The lines of a gate's output that name the errors, or its last lines if none does. */
export function errorLines(output, max) {
  const lines = output
    .split(/\r?\n/)
    .map(line => line.trimEnd())
    .filter(Boolean);
  const errors = [...new Set(lines.filter(line => ERROR_LINE.test(line)))];
  return (errors.length > 0 ? errors : lines.slice(-max)).slice(0, max);
}

function git(dir, args) {
  const result = spawnSync('git', ['-C', dir, ...args], { encoding: 'buffer', timeout: 10000, maxBuffer: 64 * 1024 * 1024 });
  return result.status === 0 ? result.stdout : null;
}

export function headCommit(dir) {
  return git(dir, ['rev-parse', 'HEAD'])?.toString('utf8').trim() ?? null;
}

/** Repository-relative paths (forward slashes) of every uncommitted change. */
function uncommittedPaths(dir) {
  const status = git(dir, ['status', '--porcelain=v1', '-z', '--untracked-files=all']);
  if (status === null) return null;
  const paths = [];
  const entries = status.toString('utf8').split('\0');
  for (let index = 0; index < entries.length; index++) {
    const entry = entries[index];
    if (entry.length < 4) continue;
    paths.push(entry.slice(3));
    // Renames and copies carry their original path as the next entry
    if (entry[0] === 'R' || entry[0] === 'C') paths.push(entries[++index]);
  }
  return paths.filter(Boolean);
}

/**
 * A fingerprint of the working tree: HEAD plus every uncommitted path and its
 * content. Equal fingerprints mean nothing changed since the last check.
 */
export function treeState(dir) {
  const head = git(dir, ['rev-parse', 'HEAD']) ?? Buffer.alloc(0);
  const paths = uncommittedPaths(dir);
  if (paths === null) return null;

  const hash = createHash('sha256').update(head);
  for (const filePath of paths) {
    try {
      hash.update(filePath).update(readFileSync(join(dir, filePath)));
    } catch {
      hash.update(filePath).update('<deleted>');
    }
  }
  return { hash: hash.digest('hex'), dirty: paths.length > 0 };
}

/**
 * Paths changed in this session: everything uncommitted plus what the commits
 * since `baseHead` touched. Without a base only the uncommitted paths count.
 */
export function changedPaths(dir, baseHead) {
  const paths = new Set(uncommittedPaths(dir) ?? []);
  if (baseHead) {
    const committed = git(dir, ['diff', '--name-only', '-z', baseHead, 'HEAD']);
    for (const path of committed?.toString('utf8').split('\0') ?? []) if (path) paths.add(path);
  }
  return [...paths].map(path => path.replaceAll('\\', '/'));
}

/** Whether a gate concerns the given changes: no `paths` means always. */
export function gateApplies(gate, paths) {
  if (!Array.isArray(gate.paths)) return true;
  return paths.some(path => gate.paths.some(prefix => path.startsWith(prefix)));
}

function stateFile(dir, sessionId) {
  const key = createHash('sha256')
    .update(`${dir}\0${sessionId ?? 'unknown'}`)
    .digest('hex')
    .slice(0, 24);
  return join(tmpdir(), 'claude-gates', `${key}.json`);
}

export function loadState(dir, sessionId) {
  try {
    return JSON.parse(readFileSync(stateFile(dir, sessionId), 'utf8'));
  } catch {
    return {};
  }
}

export function saveState(dir, sessionId, state) {
  const folder = join(tmpdir(), 'claude-gates');
  if (!existsSync(folder)) mkdirSync(folder, { recursive: true });
  writeFileSync(stateFile(dir, sessionId), JSON.stringify(state));
}

/**
 * Whether the spec skills commit without asking: on unless the developer sets
 * SPEC_AUTOCOMMIT=false (e.g. under `env` in .claude/settings.local.json).
 * Committing on someone's behalf is up to that person, so this is never read
 * from a tracked file.
 */
export function commitMode(env = process.env) {
  const value = env.SPEC_AUTOCOMMIT;
  if (value === undefined || value === '' || value === 'true') return { autoCommit: true };
  if (value === 'false') return { autoCommit: false };
  return { autoCommit: true, problem: `invalid SPEC_AUTOCOMMIT=${JSON.stringify(value)} (allowed: true, false)` };
}

export function print(output) {
  process.stdout.write(JSON.stringify(output));
}

/** The lines of a level-2 section up to the next level-2 heading. */
function sectionLines(lines, heading) {
  const start = lines.findIndex(line => line.trim().toLowerCase() === heading.toLowerCase());
  if (start < 0) return [];
  const body = lines.slice(start + 1);
  const end = body.findIndex(line => line.startsWith('## '));
  return end < 0 ? body : body.slice(0, end);
}

/**
 * One line per feature spec directly under docs/specs/ that is still
 * `status: todo` and has an `## Acceptance criteria` section: how many of its
 * criteria the Evidence table shows as built, plus the open questions that
 * keep spec-build from starting. Design documents (other status, no criteria)
 * are skipped rather than guessed at.
 */
export function openSpecs(dir) {
  const specsDir = join(dir, 'docs', 'specs');
  let names;
  try {
    names = readdirSync(specsDir).filter(name => name.endsWith('.md'));
  } catch {
    return [];
  }

  const summaries = [];
  for (const name of names.sort()) {
    let content;
    try {
      content = readFileSync(join(specsDir, name), 'utf8').replace(/\r\n/g, '\n');
    } catch {
      continue;
    }
    const frontmatter = content.match(/^---\n([\s\S]*?)\n---/);
    if (!frontmatter || !/^status:\s*todo\s*$/m.test(frontmatter[1])) continue;

    const lines = content.split('\n');
    const criteria = sectionLines(lines, '## Acceptance criteria').filter(line => /^- \[[ xX]\] AC-\d+/.test(line));
    if (criteria.length === 0) continue;
    const proven = sectionLines(lines, '## Evidence').filter(line => /^\|\s*AC-\d+\s*\|.*\|\s*(built|verified)\s*\|/.test(line));
    const openQuestions = sectionLines(lines, '## Open questions').filter(line => /^\s*[-*] \S/.test(line));

    const specName = name.slice(0, -'.md'.length);
    const open = openQuestions.length > 0 ? `, ${openQuestions.length} open questions` : '';
    // spec-build refuses a spec with open questions: they are settled in /spec first
    const next = openQuestions.length > 0 ? `/spec ${specName}` : `/spec-build ${specName}`;
    summaries.push(`${specName}: ${proven.length}/${criteria.length} ACs proven${open} -> ${next}`);
  }
  return summaries;
}
