import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const template = fileURLToPath(new URL('../skill-data/core/templates/authenticated-session.sh', import.meta.url));
const protectedUrl = 'https://app.example.com/dashboard';
const accountSignal = 'Work account dashboard';
const session = 'task-myapp-work-stable';
const key = 'a'.repeat(64);

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'authenticated-session-'));
  const bin = join(root, 'bin');
  const cwd = join(root, 'workspace');
  const log = join(root, 'calls.jsonl');
  mkdirSync(bin);
  mkdirSync(cwd);
  writeFileSync(join(bin, 'agent-browser'), `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.appendFileSync(process.env.MOCK_LOG, JSON.stringify(args) + '\\n');
const command = args.slice(9).join(' ');
console.log('sensitive-cli-output');
console.error('sensitive-cli-error');
process.exit(command === process.env.MOCK_FAIL ? 1 : 0);
`, { mode: 0o755 });
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return {
    cwd,
    run({ checkpoint = false, fail = '', env = {}, directory = cwd } = {}) {
      const result = spawnSync('/bin/bash', [template, protectedUrl, accountSignal, ...(checkpoint ? ['--checkpoint'] : [])], {
        cwd: directory,
        encoding: 'utf8',
        env: {
          ...process.env,
          PATH: `${bin}${delimiter}${process.env.PATH}`,
          AGENT_BROWSER_SESSION: session,
          AGENT_BROWSER_ENCRYPTION_KEY: undefined,
          MOCK_LOG: log,
          MOCK_FAIL: fail,
          ...env,
        },
      });
      const calls = (() => {
        try { return readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse); }
        catch (error) { if (error.code === 'ENOENT') return []; throw error; }
      })();
      const output = `${result.stdout}${result.stderr}`;
      assert.doesNotMatch(output, /sensitive-cli|aaaaaaaaaaaaaaaa/);
      if (env.AGENT_BROWSER_ENCRYPTION_KEY) {
        assert.ok(!output.includes(env.AGENT_BROWSER_ENCRYPTION_KEY));
      }
      for (const args of calls) {
        assert.deepEqual(args.slice(0, 9), [
          '--session', session, '--restore', '--restore-save', 'auto',
          '--restore-check-url', protectedUrl, '--restore-check-text', accountSignal,
        ]);
        assert.ok(!args.includes('close'));
        assert.ok(!args.includes('clear'));
        assert.ok(!args.includes('auth'));
        assert.ok(!args.includes('cookies'));
        assert.ok(!args.includes('fill'));
        if (args[9] === 'state') assert.deepEqual(args.slice(9), ['state', 'save']);
      }
      assert.deepEqual(readdirSync(cwd), []);
      return { ...result, commands: calls.map((args) => args.slice(9).join(' ')), output };
    },
  };
}

test('normal mode navigates, verifies and checkpoints the protected session without another login or export', (t) => {
  const result = fixture(t).run();
  assert.equal(result.status, 0);
  assert.deepEqual(result.commands, [
    'session info --json', `open ${protectedUrl}`, 'wait --load domcontentloaded',
    `wait --url ${protectedUrl}`, `wait --text ${accountSignal}`, 'state save',
  ]);
});

test('checkpoint preserves current page and saves immediately after readiness, URL and account checks', (t) => {
  const result = fixture(t).run({ checkpoint: true });
  assert.equal(result.status, 0);
  assert.deepEqual(result.commands, [
    'session info --json', 'wait --load domcontentloaded',
    `wait --url ${protectedUrl}`, `wait --text ${accountSignal}`, 'state save',
  ]);
});

for (const [fail, status] of [
  ['session info --json', 1],
  ['wait --load domcontentloaded', 2],
  [`wait --url ${protectedUrl}`, 2],
  [`wait --text ${accountSignal}`, 2],
]) {
  test(`checkpoint failure at ${fail} never saves or destroys old state`, (t) => {
    const result = fixture(t).run({ checkpoint: true, fail });
    assert.equal(result.status, status);
    assert.ok(!result.commands.includes('state save'));
    assert.equal(result.commands.at(-1), fail);
    if (status === 2) assert.match(result.output, /same browser.*--checkpoint/);
  });
}

for (const checkpoint of [false, true]) {
  test(`failed save in ${checkpoint ? 'checkpoint' : 'normal'} mode reports failure without a retry, close or file deletion`, (t) => {
    const result = fixture(t).run({ checkpoint, fail: 'state save' });
    assert.equal(result.status, 1);
    assert.equal(result.commands.filter((command) => command === 'state save').length, 1);
    assert.equal(result.commands.at(-1), 'state save');
    assert.doesNotMatch(result.stdout, /checkpoint saved/);
  });
}

test('navigation failure is not treated as proof login is needed', (t) => {
  const result = fixture(t).run({ fail: `open ${protectedUrl}` });
  assert.equal(result.status, 1);
  assert.equal(result.commands.at(-1), `open ${protectedUrl}`);
  assert.doesNotMatch(result.output, /complete it manually/);
});

test('unverified protected page leaves same browser open and gives manual-login guidance', (t) => {
  const result = fixture(t).run({ fail: `wait --text ${accountSignal}` });
  assert.equal(result.status, 2);
  assert.match(result.output, /loading errors or the wrong account/);
  assert.match(result.output, /same browser.*--checkpoint/);
  assert.ok(!result.commands.includes('state save'));
});

for (const env of [
  { AGENT_BROWSER_SESSION: '' },
  { AGENT_BROWSER_SESSION: 'default' },
  { AGENT_BROWSER_SESSION: '../unsafe' },
  { AGENT_BROWSER_ENCRYPTION_KEY: 'malformed-secret-do-not-print' },
]) {
  test(`rejects invalid configuration without echoing values: ${Object.keys(env).join()}`, (t) => {
    const result = fixture(t).run({ env });
    assert.equal(result.status, 1);
    assert.deepEqual(result.commands, []);
  });
}

test('missing or empty encryption key allows reuse and checkpoint without login or exports', (t) => {
  for (const encryptionKey of [undefined, '']) {
    for (const checkpoint of [false, true]) {
      const result = fixture(t).run({ checkpoint, env: { AGENT_BROWSER_ENCRYPTION_KEY: encryptionKey } });
      assert.equal(result.status, 0);
      assert.equal(result.commands[0], 'session info --json');
      assert.equal(result.commands.filter((command) => command === 'state save').length, 1);
      assert.equal(result.commands.at(-1), 'state save');
      assert.doesNotMatch(result.output, /configure|encryption key|complete it manually/i);
    }
  }
});

test('caller session and optional existing encryption key are retained from another directory', (t) => {
  const setup = fixture(t);
  const other = resolve(setup.cwd, '..');
  const result = setup.run({ checkpoint: true, directory: other, env: { AGENT_BROWSER_ENCRYPTION_KEY: key } });
  assert.equal(result.status, 0);
  assert.ok(!result.commands.some((command) => command.startsWith('session id')));
});

test('template and checkpoint docs retain validation without destructive cleanup or secret exports', () => {
  const source = readFileSync(template, 'utf8');
  assert.doesNotMatch(source, /\brm\b|cookies clear|state clear|\bclose\b|openssl|APP_PASSWORD|localStorage|auth-state\.json|STATE_FILE|\beval\b/);
  assert.doesNotMatch(source, /(?:export\s+)?AGENT_BROWSER_ENCRYPTION_KEY\s*=/);
  for (const path of [
    '../skill-data/core/SKILL.md',
    '../skill-data/core/references/authentication.md',
    '../skill-data/core/references/session-management.md',
    '../docs/content/docs/sessions.mdx',
  ]) {
    const docs = readFileSync(new URL(path, import.meta.url), 'utf8');
    assert.match(docs, /not sticky across (?:separate )?CLI calls/, path);
    assert.match(docs, /Missing checks are an error/, path);
    for (const line of docs.split('\n')) {
      if (/^agent-browser .*\bstate save$/.test(line)) {
        assert.match(line, /--restore-check-(?:url|text|fn)\s+\S+/, `${path}: ${line}`);
      }
    }
  }
});
