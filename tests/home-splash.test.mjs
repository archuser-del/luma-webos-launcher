import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import test from 'node:test';

const file = (name) => fs.readFile(new URL(`../tv/${name}`, import.meta.url), 'utf8');

test('autostart is an explicit opt-in boot hook for the public launcher id', async () => {
  const source = await file('autostart.sh');
  assert.match(source, /^APP_ID=com\.archbtw\.luma$/m);
  assert.match(source, /HOOK=\/var\/lib\/webosbrew\/init\.d\/65-luma/);
  assert.match(source, /^  enable\)$/m);
  assert.match(source, /^  disable\)$/m);
  assert.match(source, /flock -n 9/);
  assert.match(source, /luna:\/\/com\.webos\.service\.applicationmanager\/launch/);
  assert.doesNotMatch(source, /com\.ecklf\.luma/);
});

test('wake script targets the public launcher id and not the original beta app', async () => {
  const source = await file('luma-wake.sh');
  assert.match(source, /com\.archbtw\.luma/);
  assert.match(source, /luna-send/);
});
