import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

const source = await fs.readFile(new URL('../tv/home.sh', import.meta.url), 'utf8');

// tv/home.sh in a temporary root: the copied state directory with a recording luma-helper,
// the cold-boot hook as a real symlink, and stubs for the root-only id/stat checks.
async function fixture({ helper = true, relative = false, autoLaunch = 'off' } = {}) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'luma-home-wrapper-'));
  const state = path.join(root, 'state');
  const bin = path.join(root, 'bin');
  const hooks = path.join(root, 'init.d');
  const calls = path.join(root, 'calls');
  await Promise.all([state, bin, hooks].map((dir) => fs.mkdir(dir)));
  const line = 'STATE=/var/lib/luma-home\n';
  assert.ok(source.includes(line));
  const settings = path.join(root, 'settings');
  // A stand-in for the TV's settings service: getSystemSettings reports the saved value,
  // setSystemSettings records the request.
  const luna = path.join(bin, 'luna-send');
  const script = source.replace(line, `STATE='${state}'\n`).replaceAll('/usr/bin/luna-send', luna);
  await fs.writeFile(path.join(state, 'home.sh'), script, { mode: 0o755 });
  if (helper) {
    await fs.writeFile(
      path.join(state, 'luma-helper'),
      `#!/bin/sh\nfor a; do printf '[%s]' "$a"; done >>'${calls}'\necho >>'${calls}'\n`,
      { mode: 0o755 },
    );
  }
  await fs.writeFile(
    luna,
    `#!/bin/sh
case "$*" in
  *getSystemSettings*) printf '{"returnValue":true,"settings":{"homeAutoLaunch":"${autoLaunch}"}}\\n' ;;
  *setSystemSettings*) printf '%s\\n' "$*" >>'${settings}'; echo '{"returnValue":true}' ;;
esac
`,
    { mode: 0o755 },
  );
  for (const [name, body] of [
    ['id', 'echo 0'],
    ['stat', 'echo 0'],
    ['sleep', 'exit 0'],
  ]) {
    await fs.writeFile(path.join(bin, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  }
  const hook = path.join(hooks, '64-luma-home');
  await fs.symlink(relative ? '../state/home.sh' : path.join(state, 'home.sh'), hook);
  const run = (...args) =>
    spawnSync('/bin/sh', [hook, ...args], {
      encoding: 'utf8',
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
      timeout: 10000,
    });
  const recorded = async () => {
    // Long enough for a real 5 s sleep where the shell runs sleep as a builtin (busybox ash).
    for (let i = 0; i < 200; i++) {
      const text = await fs.readFile(calls, 'utf8').catch(() => '');
      if (text) return text.trim().split('\n');
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return [];
  };
  const changes = async () =>
    (await fs.readFile(settings, 'utf8').catch(() => '')).trim().split('\n').filter(Boolean);
  return { root, state, run, recorded, changes };
}

for (const relative of [false, true]) {
  test(`the ${relative ? 'relative' : 'absolute'} boot-hook symlink finds the copied helper`, async () => {
    const tv = await fixture({ relative });
    try {
      const status = tv.run('status');
      assert.equal(status.status, 0, status.stderr);
      assert.deepEqual(await tv.recorded(), [`[home][status][${tv.state}][]`]);
    } finally {
      await fs.rm(tv.root, { recursive: true, force: true });
    }
  });
}

test('a cold boot through the hook symlink schedules the copied apply', async () => {
  const tv = await fixture();
  try {
    const boot = tv.run();
    assert.equal(boot.status, 0, boot.stderr);
    assert.deepEqual(await tv.recorded(), [`[home][apply][${tv.state}][]`]);
  } finally {
    await fs.rm(tv.root, { recursive: true, force: true });
  }
});

test('a missing helper is reported before any change', async () => {
  const tv = await fixture({ helper: false });
  try {
    const status = tv.run('status');
    assert.equal(status.status, 1);
    assert.match(status.stderr, /native Luma TV helper is missing/);
  } finally {
    await fs.rm(tv.root, { recursive: true, force: true });
  }
});

test('enabling turns Home Auto Launch on so Luma opens at power-on, and disabling restores it', async () => {
  const tv = await fixture({ autoLaunch: 'off' });
  try {
    const enable = spawnSync('/bin/sh', [path.join(tv.state, 'home.sh'), 'enable'], {
      encoding: 'utf8',
      input: 'y\n',
      env: { ...process.env, PATH: `${path.join(tv.root, 'bin')}:${process.env.PATH}` },
      timeout: 10000,
    });
    assert.equal(enable.status, 0, enable.stderr);
    assert.match(enable.stdout, /Turn on Home Auto Launch/);
    assert.match(enable.stdout, /Home Auto Launch: on/);
    assert.deepEqual(await tv.recorded(), [`[home][enable][${tv.state}][]`]);
    const [turnedOn] = await tv.changes();
    assert.match(turnedOn, /"homeAutoLaunch":"on"/);
    assert.equal(
      (await fs.readFile(path.join(tv.state, 'home-auto-launch.previous'), 'utf8')).trim(),
      'off',
    );
    const disable = tv.run('disable');
    assert.equal(disable.status, 0, disable.stderr);
    assert.match(disable.stdout, /Home Auto Launch: off again/);
    assert.match((await tv.changes())[1], /"homeAutoLaunch":"off"/);
    await assert.rejects(fs.access(path.join(tv.state, 'home-auto-launch.previous')));
  } finally {
    await fs.rm(tv.root, { recursive: true, force: true });
  }
});

test('Home Auto Launch already on is left alone and not restored to anything', async () => {
  const tv = await fixture({ autoLaunch: 'on' });
  try {
    const enable = spawnSync('/bin/sh', [path.join(tv.state, 'home.sh'), 'enable'], {
      encoding: 'utf8',
      input: 'y\n',
      env: { ...process.env, PATH: `${path.join(tv.root, 'bin')}:${process.env.PATH}` },
      timeout: 10000,
    });
    assert.equal(enable.status, 0, enable.stderr);
    assert.deepEqual(await tv.changes(), []);
    const disable = tv.run('disable');
    assert.equal(disable.status, 0, disable.stderr);
    assert.deepEqual(await tv.changes(), []);
  } finally {
    await fs.rm(tv.root, { recursive: true, force: true });
  }
});
