// Luma Player's half of the debug log. While Luma's debug log is on, Luma
// passes a local address in the launch params; lines go there in small
// batches and end up in Luma's log file, tagged [player]. Lines that could
// not be sent wait in localStorage and go with the next batch, even from a
// later start of the player. Off (no address): nothing is kept or sent.
// Tokens and URL query strings never reach a line.
'use strict';

(function (root) {
  const STORE = 'lumaPlayerLog';
  const KEEP = 400;
  const EVERY_MS = 2000;

  let sink = null;
  let queue = [];
  let timer = 0;
  let sending = false;
  let storage = null;
  let Request = null;

  function pad(n, w) {
    return String(n).padStart(w || 2, '0');
  }
  // "2026-11-11T20:41:05.123-06:00": the TV's local time, as Luma writes it.
  function stamp(d) {
    d = d || new Date();
    const off = -d.getTimezoneOffset();
    const sign = off < 0 ? '-' : '+';
    const a = Math.abs(off);
    return (
      `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
      `T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` +
      `.${pad(d.getMilliseconds(), 3)}${sign}${pad(Math.floor(a / 60))}:${pad(a % 60)}`
    );
  }
  // Query strings carry api_key and X-Plex-Token; headers carry Token="…".
  function clean(text) {
    return String(text)
      .replace(/(https?:\/\/[^\s?#"'<>]+)[?#][^\s"'<>]*/g, '$1?…')
      .replace(/((?:api_key|ApiKey|X-Plex-Token|token|access_token)=)[^&\s"'<>]+/gi, '$1…')
      .replace(/(Token=")[^"]*"/g, '$1…"')
      .replace(/[\r\n]+/g, ' ');
  }
  function show(part) {
    if (part instanceof Error) return `${part.name}: ${part.message}`;
    if (part && typeof part === 'object') {
      try {
        return JSON.stringify(part);
      } catch {
        return String(part);
      }
    }
    return String(part);
  }
  function load() {
    try {
      const saved = JSON.parse((storage && storage.getItem(STORE)) || '[]');
      return Array.isArray(saved) ? saved.filter((l) => typeof l === 'string') : [];
    } catch {
      return [];
    }
  }
  function save() {
    try {
      if (!storage) return;
      if (queue.length) storage.setItem(STORE, JSON.stringify(queue));
      else storage.removeItem(STORE);
    } catch {
      /* full or private: the lines stay in memory */
    }
  }
  function schedule() {
    if (!timer && sink) timer = setTimeout(flush, EVERY_MS);
  }

  // The address Luma gave this start, or none: logging off.
  function configure(address, env) {
    env = env || {};
    storage = env.storage !== undefined ? env.storage : root.localStorage || null;
    Request = env.Request || root.XMLHttpRequest || null;
    sink =
      typeof address === 'string' && /^http:\/\/127\.0\.0\.1:\d+\//.test(address) ? address : null;
    if (!sink) {
      queue = [];
      save();
      return false;
    }
    // Lines left over from a start whose batch never arrived go first.
    const left = load();
    queue = left.length
      ? left.concat([`${stamp()} (lines above are from an earlier start)`], queue)
      : queue;
    if (queue.length > KEEP) queue = queue.slice(-KEEP);
    save();
    schedule();
    return true;
  }
  function line() {
    if (!sink) return;
    const text = Array.prototype.map
      .call(arguments, show)
      .filter((part) => part !== '')
      .join(' ');
    queue.push(`${stamp()} ${clean(text)}`);
    if (queue.length > KEEP) queue = queue.slice(-KEEP);
    save();
    schedule();
  }
  // Sends what is waiting; `done` runs either way.
  function flush(done) {
    clearTimeout(timer);
    timer = 0;
    if (!sink || !queue.length || sending || !Request) {
      if (typeof done === 'function') done();
      return;
    }
    const batch = queue.slice();
    sending = true;
    const x = new Request();
    let finished = false;
    const end = (ok) => {
      if (finished) return;
      finished = true;
      sending = false;
      if (ok) {
        queue = queue.slice(batch.length);
        save();
      }
      if (queue.length) schedule();
      if (typeof done === 'function') done();
    };
    x.onload = () => end(x.status >= 200 && x.status < 300);
    x.onerror = x.ontimeout = () => end(false);
    try {
      x.open('POST', sink);
      x.timeout = 3000;
      // text/plain: a simple request, no preflight.
      x.setRequestHeader('Content-Type', 'text/plain');
      x.send(batch.join('\n') + '\n');
    } catch {
      end(false);
    }
  }

  const api = {
    configure,
    line,
    flush,
    clean,
    stamp,
    pending: () => queue.slice(),
    active: () => !!sink,
  };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.LumaLog = api;
})(typeof window !== 'undefined' ? window : this);
