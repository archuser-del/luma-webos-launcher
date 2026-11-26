// Luma Player's debug log (extras/player/app/log.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const file = path.resolve(import.meta.dirname, '../extras/player/app/log.js');
const source = fs.readFileSync(file, 'utf8');
const SINK = 'http://127.0.0.1:41234/player-log/0123456789abcdef';

function fresh() {
  const timers = [];
  const sandbox = {
    module: { exports: {} },
    setTimeout: (fn) => timers.push(fn),
    clearTimeout: () => {},
  };
  vm.runInNewContext(source, sandbox, { filename: file });
  return { L: sandbox.module.exports, timers };
}
function storage() {
  const items = new Map();
  return {
    items,
    getItem: (k) => (items.has(k) ? items.get(k) : null),
    setItem: (k, v) => items.set(k, String(v)),
    removeItem: (k) => items.delete(k),
  };
}
// An XMLHttpRequest that answers with `status` (0: the request failed).
function requests(status, sent) {
  return class {
    open(method, url) {
      this.method = method;
      this.url = url;
    }
    setRequestHeader() {}
    send(body) {
      sent.push({ method: this.method, url: this.url, body });
      this.status = status;
      if (status) this.onload();
      else this.onerror();
    }
  };
}

test('without an address from Luma nothing is kept or sent', () => {
  const { L } = fresh();
  const store = storage();
  assert.equal(L.configure(undefined, { storage: store, Request: requests(204, []) }), false);
  L.line('video playing');
  assert.deepEqual([...L.pending()], []);
  assert.equal(store.items.size, 0);
  // Only a local address counts.
  assert.equal(L.configure('http://192.168.1.5:9/x', { storage: store }), false);
});

test('tokens and query strings never reach a line', () => {
  const { L } = fresh();
  const line = L.clean(
    'open stream http://jf:8096/Videos/1/stream.mkv?static=true&api_key=SECRET ' +
      'Token="ALSO-SECRET" X-Plex-Token=PLEX api_key=KEY',
  );
  assert.ok(!/SECRET|PLEX|KEY/.test(line), line);
  assert.match(line, /http:\/\/jf:8096\/Videos\/1\/stream\.mkv\?…/);
});

test('lines carry the local time and go to Luma in one batch', () => {
  const { L, timers } = fresh();
  const sent = [];
  L.configure(SINK, { storage: storage(), Request: requests(204, []) });
  L.configure(SINK, { storage: storage(), Request: requests(204, sent) });
  L.line('start:', 'jellyfin', { method: 'DirectPlay' });
  L.line('video playing at 0:00');
  assert.ok(timers.length > 0, 'a flush is scheduled');
  let done = false;
  L.flush(() => (done = true));
  assert.ok(done);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].method, 'POST');
  assert.equal(sent[0].url, SINK);
  const lines = sent[0].body.trim().split('\n');
  assert.equal(lines.length, 2);
  assert.match(
    lines[0],
    /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}[+-]\d\d:\d\d start: jellyfin \{"method":"DirectPlay"\}$/,
  );
  assert.deepEqual([...L.pending()], [], 'sent lines are gone');
});

test('lines Luma did not receive wait for the next start of the player', () => {
  const store = storage();
  const first = fresh().L;
  first.configure(SINK, { storage: store, Request: requests(0, []) });
  first.line('video error 3 (decode)');
  first.flush();
  assert.equal(first.pending().length, 1, 'kept after a failed send');
  assert.ok(store.items.get('lumaPlayerLog').includes('video error 3'));

  // The player starts again, Luma is listening now.
  const sent = [];
  const second = fresh().L;
  second.configure(SINK, { storage: store, Request: requests(204, sent) });
  second.line('start: jellyfin');
  second.flush();
  const body = sent[0].body;
  assert.ok(body.indexOf('video error 3') < body.indexOf('earlier start'));
  assert.ok(body.indexOf('earlier start') < body.indexOf('start: jellyfin'));
  assert.equal(store.items.has('lumaPlayerLog'), false, 'nothing left over');
});

test('at most 400 lines wait, the newest ones', () => {
  const { L } = fresh();
  L.configure(SINK, { storage: storage(), Request: requests(0, []) });
  for (let i = 0; i < 450; i++) L.line(`line ${i}`);
  const waiting = L.pending();
  assert.equal(waiting.length, 400);
  assert.match(waiting[399], /line 449$/);
});

test('the player loads its log before itself', () => {
  const html = fs.readFileSync(
    path.resolve(import.meta.dirname, '../extras/player/app/index.html'),
    'utf8',
  );
  assert.ok(html.indexOf('log.js') < html.indexOf('player.js'));
});
