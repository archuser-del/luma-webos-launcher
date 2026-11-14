// Luma player renders controls over the native webOS video element.
// Signed media server stream URLs remain in memory, never logged.
'use strict';

// Debug log (log.js): a no-op unless Luma's debug log is on.
function plog() {
  if (window.LumaLog) window.LumaLog.line.apply(null, arguments);
}
window.addEventListener('unhandledrejection', (e) => plog('unhandled rejection:', e.reason));
window.onerror = (message, source, line) => {
  plog(`script error: ${message} (${String(source).split('/').pop()}:${line})`);
  const n = document.getElementById('note');
  if (n) {
    n.classList.remove('hidden');
    n.classList.add('error');
  }
  const t = document.getElementById('noteTitle'),
    s = document.getElementById('noteSub');
  if (t) t.textContent = 'Player error';
  if (s) s.textContent = `${message} (line ${line})`;
};

const $ = (id) => document.getElementById(id);
const video = $('video');
const LUMA = 'com.archbtw.luma';
const PLAYER_VERSION = '0.4.3';
const HIDE_MS = 4000;
const SEEK_STEP = 10;
const SEEK_SETTLE_MS = 450;
const MARK_EVERY_MS = 30000;
const PAD = 88;
const WIDE = 1744;

let p = {}; // launch params from Luma
let icons = {}; // name -> svg text
let focus = 'bar'; // 'close' | 'bar' | 'row:N' | 'panelClose' | 'opt:KIND:N'
let rowItems = []; // [{id, icon, label, trailing, act}]
let panel = null; // null | 'tracks' | 'quality'
let audioIndex = 0;
let subtitleIndex = -1;
let qualityIndex = 0;
let cues = [];
let lastInput = Date.now();
let pendingSeek = null; // seconds, applied after SEEK_SETTLE_MS
let seekTimer = 0;
let scrubbing = false;
let hoverSec = null;
let playing = false;
let lastMark = 0;
let leaving = false;
// Jellyfin: what the TV decodes, the file's streams, the choices made and
// the stream chosen from them (streams.js). null for other sources.
let jf = null;

// ---------- helpers ----------
function clock(sec) {
  sec = Math.max(0, Math.floor(sec || 0));
  const h = Math.floor(sec / 3600),
    m = Math.floor(sec / 60) % 60,
    s = sec % 60;
  const ss = String(s).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}
function hm(d) {
  return `${d.getHours()}:${String(d.getMinutes()).padStart(2, '0')}`;
}
function pos() {
  return video.currentTime || 0;
}
function duration() {
  const d = video.duration;
  return isFinite(d) && d > 0 ? d : (p.durationMs || 0) / 1000;
}
function now() {
  return pendingSeek !== null ? pendingSeek : pos();
}
function icon(name) {
  return icons[name] || '';
}
function el(tag, cls, html) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (html !== undefined) e.innerHTML = html;
  return e;
}
function loadIcons(names, done) {
  let left = names.length;
  names.forEach((n) => {
    const x = new XMLHttpRequest();
    x.open('GET', `icons/${n}.svg`);
    x.onload = x.onerror = () => {
      icons[n] = (x.responseText || '')
        .replace(/ width="\d+" height="\d+"/, '')
        .replace(/ color="[^"]*"/, '');
      if (--left === 0) done();
    };
    x.send();
  });
}
function luna(uri, payload) {
  try {
    const bridge = new PalmServiceBridge();
    bridge.onservicecallback = () => {};
    bridge.call(uri, JSON.stringify(payload));
  } catch {
    /* desktop browser */
  }
}
// ---------- accent ----------
function hex(v, fallback) {
  return /^#?[0-9a-f]{6}$/i.test(v || '') ? (v[0] === '#' ? v : '#' + v) : fallback;
}
function mixHex(a, b, t) {
  const n = (h, i) => parseInt(h.substr(1 + 2 * i, 2), 16);
  const c = [0, 1, 2].map((i) =>
    Math.round(n(a, i) * (1 - t) + n(b, i) * t)
      .toString(16)
      .padStart(2, '0'),
  );
  return '#' + c.join('');
}
function applyAccent() {
  const a = hex(p.accentA, '#7ee8a5');
  const b = p.accentKind === 'gradient' ? hex(p.accentB, a) : a;
  const s = document.documentElement.style;
  s.setProperty('--accent-a', a);
  s.setProperty('--accent-b', b);
  s.setProperty('--accent-mid', mixHex(a, b, 0.55));
  s.setProperty('--accent-ink', hex(p.accentInk, '#121519'));
}

// ---------- controls visibility ----------
function reveal() {
  lastInput = Date.now();
  document.body.classList.add('controls');
}
function tick() {
  const keep = panel || video.paused || scrubbing || !playing;
  if (!keep && Date.now() - lastInput > HIDE_MS) document.body.classList.remove('controls');
  render();
  renderMusic();
  if (playing && !video.paused && Date.now() - lastMark > MARK_EVERY_MS) mark();
}

// ---------- rendering ----------
function render() {
  const d = duration(),
    t = now();
  const share = d ? Math.min(1, Math.max(0, t / d)) : 0;
  $('filled').style.width = share * WIDE + 'px';
  $('thumb').style.left = share * WIDE + 'px';
  let buffered = 0;
  for (let i = 0; i < video.buffered.length; i++) {
    if (video.buffered.start(i) <= video.currentTime + 1)
      buffered = Math.max(buffered, video.buffered.end(i));
  }
  $('buffered').style.width = (d ? Math.min(1, buffered / d) * WIDE : 0) + 'px';
  $('pos').textContent = clock(t);
  $('remain').textContent = '−' + clock(d - t);
  const date = new Date();
  $('clock').textContent = hm(date);
  $('ends').textContent = d ? 'Ends at ' + hm(new Date(date.getTime() + (d - t) * 1000)) : '';
  $('bar').classList.toggle('focused', focus === 'bar' && !panel);
  const chip = $('chip');
  if (hoverSec !== null && d) {
    chip.classList.remove('hidden');
    chip.textContent = clock(hoverSec);
    chip.style.left = Math.min(WIDE - 40, Math.max(40, (hoverSec / d) * WIDE)) + 'px';
  } else chip.classList.add('hidden');
  const left = d - t;
  const next = $('nextIn');
  if (p.next && d && left <= 30 && left > 0) {
    next.classList.remove('hidden');
    next.textContent = `Next episode in ${Math.ceil(left)}s`;
  } else next.classList.add('hidden');
  // Subtitles from the downloaded file.
  const cap = $('caption');
  const ms = pos() * 1000;
  const cue = subtitleIndex >= 0 ? cues.find((c) => c.start <= ms && c.end >= ms) : null;
  if (cue) {
    cap.textContent = cue.text;
    cap.classList.remove('hidden');
  } else cap.classList.add('hidden');
  $('badgeTime').textContent = clock(t);
  $('badgeIcon').innerHTML = icon(video.paused ? 'pause' : 'play');
  paintFocus();
}
function paintFocus() {
  document.querySelectorAll('.focus').forEach((e) => e.classList.remove('focus'));
  const target = document.querySelector(`[data-focus="${focus}"]`);
  if (target) target.classList.add('focus');
}
function header() {
  const series = p.source === 'series';
  $('kind').textContent = series ? 'SERIES' : 'MOVIE';
  $('mode').textContent = jf && jf.plan ? jf.plan.label.toUpperCase() : 'MEDIA';
  const title = $('title');
  title.textContent = p.title || '';
  title.classList.toggle('long', (p.title || '').length > 42);
  $('episode').textContent = p.episodeLine || '';
  $('close').innerHTML = icon('close');
  $('close').dataset.focus = 'close';
  $('panelClose').innerHTML = icon('close');
  $('panelClose').dataset.focus = 'panelClose';
}
function buildRow() {
  rowItems = [];
  if ((p.audio || []).length > 1 || (p.subtitles || []).length) {
    rowItems.push({
      icon: 'volume',
      label: audioName(audioIndex),
      act: () => openPanel('tracks', 'audio'),
    });
  }
  if ((p.subtitles || []).length) {
    rowItems.push({
      icon: 'captions',
      label: subtitleIndex >= 0 ? p.subtitles[subtitleIndex].name : 'Subtitles off',
      act: () => openPanel('tracks', 'sub'),
    });
  }
  if ((p.qualities || []).length > 1) {
    rowItems.push({
      icon: 'quality',
      label: p.qualities[qualityIndex].name,
      act: () => openPanel('quality', 'quality'),
    });
  }
  const right = [];
  if (p.source === 'series') {
    if (p.prev)
      right.push({
        icon: 'skip-back',
        label: `Episode ${p.prev.number}`,
        act: () => episode(p.prev),
      });
    if (p.next)
      right.push({
        icon: 'skip-forward',
        label: `Episode ${p.next.number}`,
        trailing: true,
        act: () => episode(p.next),
      });
  } else {
    right.push({ icon: 'refresh', label: 'From start', act: () => seekTo(0, true) });
  }
  const row = $('row');
  row.innerHTML = '';
  let x = PAD;
  const make = (item, i) => {
    const b = el('div', 'btn');
    b.dataset.focus = 'row:' + i;
    b.dataset.index = i;
    b.innerHTML = item.trailing
      ? `<span>${esc(item.label)}</span>${icon(item.icon)}`
      : `${icon(item.icon)}<span>${esc(item.label)}</span>`;
    row.appendChild(b);
    return b;
  };
  rowItems.forEach((item, i) => {
    const b = make(item, i);
    b.style.left = x + 'px';
    x += b.offsetWidth + 14;
  });
  const start = rowItems.length;
  rowItems = rowItems.concat(right);
  const widths = right.map((item, j) => {
    const b = make(item, start + j);
    return b;
  });
  let rx =
    PAD +
    WIDE -
    widths.reduce((s, b) => s + b.offsetWidth, 0) -
    14 * Math.max(0, widths.length - 1);
  widths.forEach((b) => {
    b.style.left = rx + 'px';
    rx += b.offsetWidth + 14;
  });
  if (focus.startsWith('row:') && +focus.slice(4) >= rowItems.length) focus = 'bar';
}
function esc(s) {
  return String(s).replace(
    /[&<>"]/g,
    (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c],
  );
}

// ---------- tracks ----------
function audioList() {
  if (jf) return jf.info.audio.map((a) => a.name);
  const t = video.audioTracks;
  const n = t && t.length ? t.length : (p.audio || []).length;
  const out = [];
  for (let i = 0; i < n; i++) out.push(audioName(i));
  return out;
}
// The stream's own label first ("01. Дубляж. MovieDalen (RUS)"); media's
// list may be ordered differently from the playlist.
function audioName(i) {
  if (jf) return (jf.info.audio[i] || {}).name || 'Audio';
  const t = video.audioTracks && video.audioTracks[i];
  const label = t && (t.label || '').trim();
  if (label) return label.replace(/^\d+\.\s*/, '');
  return (p.audio || [])[i] || (t && t.language) || `Track ${i + 1}`;
}
// Turn on one of the file's own audio tracks (Direct Play keeps them all).
function enableTrack(i) {
  const t = video.audioTracks;
  if (t && t.length > i) for (let j = 0; j < t.length; j++) t[j].enabled = j === i;
}
function setAudio(i) {
  plog(`choose audio ${i}:`, audioName(i));
  if (jf) {
    const a = jf.info.audio[i];
    if (!a) return;
    audioIndex = i;
    jf.audio = a.index;
    jfReplan();
    buildRow();
    return;
  }
  const t = video.audioTracks;
  if (t && t.length) for (let j = 0; j < t.length; j++) t[j].enabled = j === i;
  audioIndex = i;
  buildRow();
}
function setSubtitle(i) {
  plog(
    `choose subtitle ${i}:`,
    i >= 0 && p.subtitles && p.subtitles[i] ? p.subtitles[i].name : 'off',
  );
  subtitleIndex = i;
  cues = [];
  const track = (p.subtitles || [])[i];
  if (jf) {
    // An image subtitle is burned into the picture; leaving one, or
    // picking one, needs another stream from Jellyfin.
    const wasBurned = jf.plan && jf.plan.burn;
    jf.subtitle = track ? track.index : null;
    if ((track && track.image) || wasBurned) jfReplan();
  }
  if (track && track.url) {
    const x = new XMLHttpRequest();
    x.open('GET', track.url);
    x.onload = () => {
      if (subtitleIndex === i) cues = parseSubtitles(x.responseText || '');
    };
    x.send();
  }
  buildRow();
}
function parseStamp(s) {
  const m = /(?:(\d+):)?(\d+):(\d+)[.,](\d+)/.exec(s);
  if (!m) return null;
  return ((+(m[1] || 0) * 60 + +m[2]) * 60 + +m[3]) * 1000 + +(m[4] + '00').slice(0, 3);
}
function parseSubtitles(text) {
  const out = [];
  text
    .replace(/\r\n/g, '\n')
    .split(/\n\n+/)
    .forEach((block) => {
      const lines = block.split('\n');
      const i = lines.findIndex((l) => l.indexOf('-->') >= 0);
      if (i < 0) return;
      const [a, b] = lines[i].split('-->');
      const start = parseStamp(a),
        end = parseStamp(b);
      const body = lines
        .slice(i + 1)
        .join('\n')
        .replace(/<[^>]+>/g, '')
        .replace(/\{[^}]*\}/g, '')
        .trim();
      if (start !== null && end !== null && body) out.push({ start, end, text: body });
    });
  return out.sort((x, y) => x.start - y.start);
}
function setQuality(i) {
  plog(`choose quality ${i}:`, p.qualities && p.qualities[i] ? p.qualities[i].name : '?');
  if (jf) {
    const q = (p.qualities || [])[i];
    if (!q) return;
    qualityIndex = i;
    jf.quality = q.bitrate ? { bitrate: q.bitrate, height: q.height } : null;
    jfReplan();
    buildRow();
    return;
  }
  const q = (p.qualities || [])[i];
  if (!q || !q.url || i === qualityIndex) {
    qualityIndex = i;
    buildRow();
    return;
  }
  const at = video.currentTime;
  qualityIndex = i;
  open(q.url, at);
  buildRow();
}

// ---------- panel ----------
function openPanel(kind, column) {
  panel = kind;
  $('tracks').classList.remove('hidden');
  $('panelTitle').textContent = kind === 'quality' ? 'Quality' : 'Audio and subtitles';
  const cols = $('cols');
  cols.innerHTML = '';
  const inner = 1120 - 96;
  const defs =
    kind === 'quality'
      ? [
          {
            key: 'quality',
            title: 'QUALITY',
            rows: p.qualities.map((q) => q.name),
            chosen: qualityIndex,
            x: 48,
            w: inner,
          },
        ]
      : [
          {
            key: 'audio',
            title: 'AUDIO',
            rows: audioList(),
            chosen: audioIndex,
            x: 48,
            w: (inner - 40) / 2,
          },
          {
            key: 'sub',
            title: 'SUBTITLES',
            rows: ['Off'].concat((p.subtitles || []).map((s) => s.name)),
            chosen: subtitleIndex + 1,
            x: 48 + (inner - 40) / 2 + 40,
            w: (inner - 40) / 2,
          },
        ];
  defs.forEach((d) => {
    if (kind !== 'quality' && d.key === 'sub' && !(p.subtitles || []).length) return;
    const col = el('div', 'col');
    col.style.left = d.x + 'px';
    col.style.width = d.w + 'px';
    const count = d.key === 'sub' ? d.rows.length - 1 : d.rows.length;
    col.appendChild(el('h3', '', `${d.title} · ${count}`));
    const list = el('div', 'list');
    list.style.maxHeight = 68 * 8 + 'px';
    list.style.overflow = 'hidden';
    d.rows.forEach((name, i) => {
      const o = el('div', 'opt', `<span>${esc(name)}</span>${i === d.chosen ? icon('check') : ''}`);
      o.dataset.focus = `opt:${d.key}:${i}`;
      list.appendChild(o);
    });
    col.appendChild(list);
    cols.appendChild(col);
  });
  const chosen =
    column === 'sub' ? subtitleIndex + 1 : column === 'quality' ? qualityIndex : audioIndex;
  focus = `opt:${column}:${chosen}`;
  scrollToFocus();
  reveal();
  render();
}
function closePanel() {
  const was = panel;
  panel = null;
  $('tracks').classList.add('hidden');
  const at = rowItems.findIndex((r) =>
    was === 'quality' ? r.icon === 'quality' : r.icon === 'volume' || r.icon === 'captions',
  );
  focus = at >= 0 ? 'row:' + at : 'bar';
  reveal();
  render();
}
function chooseOption(kind, i) {
  if (kind === 'audio') setAudio(i);
  else if (kind === 'sub') setSubtitle(i - 1);
  else setQuality(i);
  openPanel(panel, kind);
}
function optCount(kind) {
  return document.querySelectorAll(`[data-focus^="opt:${kind}:"]`).length;
}
function scrollToFocus() {
  const e = document.querySelector(`[data-focus="${focus}"]`);
  if (e && e.parentNode.classList.contains('list')) {
    const list = e.parentNode;
    const top = e.offsetTop - list.offsetTop;
    if (top < list.scrollTop) list.scrollTop = top;
    if (top + 68 > list.scrollTop + list.clientHeight)
      list.scrollTop = top + 68 - list.clientHeight;
  }
}

// ---------- Jellyfin ----------
// The TV is asked once per title what it decodes; streams.js turns that
// and the viewer's choices into Direct Play, Direct Stream or a transcode.
function jfStart(at) {
  const j = p.jellyfin;
  const S = window.LumaStreams;
  const info = S.sourceInfo(j.source);
  jf = {
    caps: S.capabilities(video, p.audioSupport),
    info,
    audio: typeof j.audio === 'number' ? j.audio : null,
    subtitle: typeof j.subtitle === 'number' ? j.subtitle : null,
    quality: j.quality || null,
    forceStream: false,
    fallback: 0,
    plan: null,
  };
  const chosenAudio = info.audio.findIndex((a) => a.index === jf.audio);
  audioIndex =
    chosenAudio >= 0
      ? chosenAudio
      : Math.max(
          0,
          info.audio.findIndex((a) => a.isDefault),
        );
  p.audio = info.audio.map((a) => a.name);
  const yes = (o) => Object.keys(o).filter((k) => o[k]);
  plog('TV decodes video:', yes(jf.caps.video).join(','), '· audio:', yes(jf.caps.audio).join(','));
  const support = p.audioSupport || {};
  plog(
    `TrueHD: TV says ${jf.caps.asked.truehd ? 'yes' : 'no'}, setting ${support.truehd || 'auto'}`,
    `· DTS: TV says ${jf.caps.asked.dts ? 'yes' : 'no'}, setting ${support.dts || 'auto'}`,
  );
  plog(
    'file:',
    info.container,
    info.video.codec,
    info.video.rangeType || (info.video.hdr ? 'HDR' : 'SDR'),
    `${info.video.height || '?'}p`,
    '· audio tracks:',
    info.audio.map((a) => a.codec + (a.isDefault ? '*' : '')).join(','),
    '· subtitles:',
    info.subtitles.length,
  );
  const ctx = jfContext();
  p.subtitles = info.subtitles.map((sub) => ({
    name: S.subtitleLabel(info, sub),
    url: S.subtitleUrl(ctx, sub),
    image: sub.image,
    index: sub.index,
  }));
  p.qualities = [{ name: info.video.hdr ? 'Original · HDR' : 'Original' }].concat(
    S.qualities(info),
  );
  qualityIndex = jf.quality
    ? Math.max(
        0,
        p.qualities.findIndex((q) => q.bitrate === jf.quality.bitrate),
      )
    : 0;
  const sub = p.subtitles.findIndex((t) => t.index === jf.subtitle);
  subtitleIndex = -1;
  cues = [];
  jf.plan = S.plan(jfContext());
  logPlan('plan');
  header();
  buildRow();
  if (sub >= 0) {
    subtitleIndex = sub;
    const track = p.subtitles[sub];
    if (track.url) setSubtitle(sub);
  }
  open(jf.plan.url, at);
}
function jfContext() {
  const j = p.jellyfin;
  return {
    base: j.base,
    token: j.token,
    deviceId: j.deviceId,
    item: j.item,
    session: j.session,
    info: jf.info,
    caps: jf.caps,
    audio: jf.audio,
    subtitle: jf.subtitle,
    quality: jf.quality,
    forceStream: jf.forceStream,
    fallback: jf.fallback,
  };
}
// A choice changed: the same stream only switches tracks; another stream
// reopens where the picture was.
function logPlan(why) {
  const q = jf.plan;
  plog(
    `${why}:`,
    q.method,
    '·',
    q.label,
    q.reasons && q.reasons.length ? `· because ${q.reasons.join(', ')}` : '',
  );
}
function jfReplan() {
  const at = pos();
  const next = window.LumaStreams.plan(jfContext());
  const old = jf.plan;
  jf.plan = next;
  logPlan('new plan');
  header();
  if (old && old.url === next.url) {
    enableTrack(next.audioPosition);
    return;
  }
  open(next.url, at);
}
// The next or previous episode: Jellyfin is asked for its file first.
function jfEpisode(e, done) {
  const r = p.report;
  const x = new XMLHttpRequest();
  x.open(
    'POST',
    `${r.base}/Items/${e.item}/PlaybackInfo?UserId=${encodeURIComponent(r.user || '')}`,
  );
  x.setRequestHeader('Content-Type', 'application/json');
  x.setRequestHeader(
    'Authorization',
    `MediaBrowser Client="Luma", Device="LG TV", DeviceId="${r.deviceId}", Version="${r.version || '1'}", Token="${r.token}"`,
  );
  x.onload = () => {
    let info = null;
    try {
      info = JSON.parse(x.responseText || 'null');
    } catch {
      info = null;
    }
    const source = info && info.MediaSources && info.MediaSources[0];
    if (x.status >= 300 || !source) return done(false);
    p.jellyfin = Object.assign({}, p.jellyfin, {
      item: e.item,
      source,
      session: info.PlaySessionId || p.jellyfin.session,
      audio: null,
      subtitle: null,
      quality: null,
    });
    r.source = source.Id;
    r.session = p.jellyfin.session;
    done(true);
  };
  x.onerror = () => done(false);
  x.send(JSON.stringify({ UserId: r.user || undefined }));
}

// ---------- playback ----------
let opening = null;
// The stream's settings for the log: what Jellyfin is asked to copy or
// convert. The address and its keys stay out (log.js drops query strings).
const STREAM_KEYS = [
  'static',
  'SegmentContainer',
  'StartTimeTicks',
  'VideoCodec',
  'AllowVideoStreamCopy',
  'AudioCodec',
  'AudioStreamIndex',
  'TranscodingMaxAudioChannels',
  'SubtitleStreamIndex',
  'SubtitleMethod',
  'VideoBitrate',
  'MaxHeight',
];
function streamSettings(url) {
  try {
    const q = new URL(url).searchParams;
    return STREAM_KEYS.filter((k) => q.has(k))
      .map((k) => `${k}=${q.get(k)}`)
      .join(' ');
  } catch {
    return '';
  }
}
function open(url, at) {
  plog('open stream', url, at > 0 ? `at ${Math.round(at)} s` : '', streamSettings(url));
  playing = false;
  showNote('Loading…');
  // Tear the previous stream down first: on webOS a bare src swap can leave
  // the old media pipeline playing next to the new one.
  if (opening) video.removeEventListener('loadedmetadata', opening);
  video.pause();
  video.removeAttribute('src');
  video.load();
  video.src = url;
  const start = () => {
    video.removeEventListener('loadedmetadata', start);
    opening = null;
    if (at > 0) video.currentTime = at;
    if (jf) enableTrack(jf.plan ? jf.plan.audioPosition : 0);
    else if (video.audioTracks && video.audioTracks.length > audioIndex) setAudio(audioIndex);
  };
  opening = start;
  video.addEventListener('loadedmetadata', start);
  const started = video.play();
  if (started && started.catch) started.catch(() => {});
  if (p.report) setTimeout(() => report('start'), 0);
}
function showNote(title, sub) {
  // Music keeps its screen: the state goes into the kicker line instead.
  if (music) {
    $('mState').textContent = sub ? "CAN'T PLAY THIS SONG" : 'LOADING';
    $('mState').classList.toggle('error', !!sub);
    return;
  }
  $('note').classList.remove('hidden');
  $('note').classList.toggle('error', !!sub);
  $('noteTitle').textContent = title;
  $('noteSub').textContent = sub || '';
}
function seekBy(delta) {
  const d = duration();
  const base = now();
  seekTo(Math.max(0, Math.min(d ? d - 1 : base + delta, base + delta)), false);
}
// D-pad and wheel steps land together: the picture follows once they stop.
function seekTo(sec, immediate) {
  pendingSeek = sec;
  clearTimeout(seekTimer);
  const apply = () => {
    video.currentTime = pendingSeek;
    pendingSeek = null;
  };
  if (immediate) apply();
  else seekTimer = setTimeout(apply, SEEK_SETTLE_MS);
  reveal();
  render();
}
function toggle() {
  if (video.paused) {
    // A stream that failed has nothing to play: no unhandled rejection.
    const started = video.play();
    if (started && started.catch) started.catch(() => {});
  } else {
    video.pause();
    mark();
  }
  reveal();
}
// Jellyfin and Plex keep the watch position themselves; tokens never leave memory.
// event: 'start' | 'progress' | 'stop'.
function report(event, done) {
  const r = p.report;
  if (!r || !r.base) {
    if (done) done();
    return;
  }
  const ms = Math.floor(pos() * 1000);
  const x = new XMLHttpRequest();
  x.onloadend = () => {
    if (x.status < 200 || x.status >= 300) {
      plog(`progress report "${event}" to ${r.service || 'jellyfin'} failed: HTTP ${x.status}`);
    }
    if (done) done();
  };
  if (r.service === 'plex') {
    const state = event === 'stop' ? 'stopped' : video.paused ? 'paused' : 'playing';
    const total = Math.floor((duration() || 0) * 1000);
    x.open(
      'GET',
      `${r.base}/:/timeline?ratingKey=${r.item}&key=%2Flibrary%2Fmetadata%2F${r.item}&state=${state}&time=${ms}&duration=${total}&X-Plex-Token=${r.token}`,
    );
    x.setRequestHeader('X-Plex-Client-Identifier', r.deviceId);
    x.setRequestHeader('X-Plex-Product', 'Luma');
    x.setRequestHeader('X-Plex-Session-Identifier', r.session);
    x.send();
    return;
  }
  const path = {
    start: '/Sessions/Playing',
    progress: '/Sessions/Playing/Progress',
    stop: '/Sessions/Playing/Stopped',
  }[event];
  x.open('POST', r.base + path);
  x.setRequestHeader('Content-Type', 'application/json');
  x.setRequestHeader(
    'Authorization',
    `MediaBrowser Client="Luma", Device="LG TV", DeviceId="${r.deviceId}", Version="${r.version || '1'}", Token="${r.token}"`,
  );
  x.send(
    JSON.stringify({
      ItemId: r.item,
      MediaSourceId: r.source,
      PlaySessionId: r.session,
      EventName: 'TimeUpdate',
      PositionTicks: ms * 10000,
      IsPaused: video.paused,
      CanSeek: true,
    }),
  );
}
function mark(done) {
  lastMark = Date.now();
  if (p.report) report('progress', done);
  else if (done) done();
}
function episode(target) {
  if (target && p.report) {
    const list = p.report.episodes || [];
    const i = list.findIndex((e) => e.season === target.season && e.number === target.number);
    if (i < 0) {
      showNote("Can't play this video", 'Episode not found');
      return;
    }
    const e = list[i];
    report('stop');
    const key = (x) => x && { season: x.season, number: x.number };
    p.report.item = e.item;
    if (p.report.service === 'jellyfin') p.report.source = e.item;
    p.season = e.season;
    p.video = e.number;
    p.prev = key(list[i - 1]);
    p.next = key(list[i + 1]);
    p.episodeLine = e.line;
    p.qualities = [];
    p.audio = [];
    p.subtitles = [];
    subtitleIndex = -1;
    cues = [];
    if (jf) {
      showNote('Loading…');
      jfEpisode(e, (ok) => {
        if (ok) return jfStart(0);
        jf = null;
        header();
        buildRow();
        open(e.url, 0);
      });
      return;
    }
    header();
    buildRow();
    open(e.url, 0);
    return;
  }
}
function leave(why) {
  if (leaving) return;
  leaving = true;
  plog('close:', why || 'back', `at ${clock(pos())}`);
  if (window.LumaLog) window.LumaLog.flush();
  video.pause();
  // Luma reloads the card on return, so the watch time is saved first (1.5 s at most).
  let gone = false;
  const go = () => {
    if (gone) return;
    gone = true;
    luna('luna://com.webos.applicationManager/launch', {
      id: LUMA,
      params: { playerClosed: true, id: p.id || 0 },
    });
    setTimeout(() => {
      try {
        window.close();
      } catch {}
    }, 300);
  };
  if (p.report) report('stop', go);
  else mark(go);
  setTimeout(go, 1500);
}

video.addEventListener('playing', () => {
  playing = true;
  $('note').classList.add('hidden');
  if (music) {
    $('mState').textContent = 'NOW PLAYING';
    $('mState').classList.remove('error');
  }
});
video.addEventListener('waiting', () => {
  if (playing) render();
});
// For the debug log: what Jellyfin answers at the address that failed (its
// status, and for HLS the variant lines with their CODECS).
function probe(url) {
  if (!url || !window.LumaLog || !window.LumaLog.active()) return;
  const x = new XMLHttpRequest();
  x.onload = () => {
    const text = String(x.responseText || '').slice(0, 4000);
    const lines = /m3u8/.test(url)
      ? text.split('\n').filter((l) => /^#EXT-X-(STREAM-INF|MEDIA|VERSION|MAP)/.test(l))
      : [];
    plog(`server answered HTTP ${x.status}`, lines.slice(0, 4).join(' | '));
  };
  x.onerror = () => plog('server did not answer');
  try {
    x.open('GET', url);
    // A peek is enough for a video file.
    if (!/m3u8/.test(url)) x.setRequestHeader('Range', 'bytes=0-1023');
    x.send();
  } catch {
    plog('server could not be asked');
  }
}

// What the TV's media pipeline does, for the debug log.
['loadstart', 'canplay', 'stalled', 'pause', 'seeked', 'emptied', 'abort'].forEach((name) =>
  video.addEventListener(name, () => plog(`video ${name} at ${clock(video.currentTime)}`)),
);
video.addEventListener('loadedmetadata', () => {
  plog(
    `video metadata: ${video.videoWidth}x${video.videoHeight}, ${clock(video.duration)},`,
    `${video.audioTracks ? video.audioTracks.length : '?'} audio tracks`,
  );
  // Copied video keeps the file's size; a much smaller picture means the
  // server is converting it.
  const wanted = jf && jf.plan && jf.plan.method !== 'Transcode' && jf.info.video.height;
  if (wanted && video.videoHeight && video.videoHeight < wanted * 0.75) {
    plog(`warning: the server sends ${video.videoHeight}p, the file is ${wanted}p (converted?)`);
  }
});
video.addEventListener('playing', () => plog(`video playing at ${clock(video.currentTime)}`));
video.addEventListener('waiting', () => plog(`video buffering at ${clock(video.currentTime)}`));
video.addEventListener('ended', () => plog('video ended'));
const MEDIA_ERRORS = ['', 'aborted', 'network', 'decode', 'format not supported'];
video.addEventListener('error', () => {
  // The TV refused the original file after all: retry once with the video
  // copied into a new container (still no re-encoding, HDR intact).
  // Only a format or decode refusal: a network hiccup is not the file's fault.
  const code = video.error ? video.error.code : 0;
  const detail = video.error && video.error.message ? `: ${video.error.message}` : '';
  plog(`video error ${code} (${MEDIA_ERRORS[code] || 'unknown'})${detail}`);
  if (jf && jf.plan && jf.plan.method === 'DirectPlay' && !jf.forceStream && code >= 3) {
    jf.forceStream = true;
    // Set to "TV plays it", but this TV refused: convert that codec now.
    const track = jf.info.audio.find((a) => a.index === jf.audio) || jf.info.audio[audioIndex];
    const kind = track && window.LumaStreams.family(track.codec);
    if (kind === 'truehd' || kind === 'dts') {
      jf.caps.audio[kind] = false;
      plog(`${kind === 'truehd' ? 'TrueHD' : 'DTS'} refused after all: converting it`);
    }
    plog('Direct Play refused: retrying as Direct Stream');
    jfReplan();
    return;
  }
  if (jfFallback('refused')) return;
  plog('gave up: "The stream did not open"');
  probe(video.currentSrc || video.src);
  showNote("Can't play this video", 'The stream did not open');
});
// The TV refused a converted stream, or it broke off: the same video copy
// with plainer audio (E-AC-3, then AC-3, then AAC). False when none is left.
function jfFallback(why) {
  if (!jf || !jf.plan || !jf.plan.fallbackLeft) return false;
  jf.fallback = (jf.fallback || 0) + 1;
  plog(`stream ${why}: retrying with plainer audio`);
  jfReplan();
  return true;
}
video.addEventListener('ended', () => {
  // An end long before the film's own is the stream breaking off, not the
  // film finishing: try again from there rather than closing.
  const total = (p.durationMs || 0) / 1000;
  if (!music && jf && total > 0 && pos() < total - 60) {
    plog(`stream ended early at ${clock(pos())} of ${clock(total)}`);
    if (!jfFallback('broke off')) {
      showNote("Can't play this video", 'The stream stopped early');
    }
    return;
  }
  if (music) {
    if (music.at + 1 < music.order.length) step(1);
    else {
      report('stop');
      focus = 'm:play';
      renderMusic();
    }
    return;
  }
  mark();
  if (p.next) episode(p.next);
  else leave('ended');
});

// ---------- remote: D-pad ----------
function move(dir) {
  if (panel) return movePanel(dir);
  if (focus === 'close') {
    if (dir === 'down') focus = 'bar';
    return;
  }
  if (focus === 'bar') {
    if (dir === 'left') seekBy(-SEEK_STEP);
    else if (dir === 'right') seekBy(SEEK_STEP);
    else if (dir === 'up') focus = 'close';
    else if (dir === 'down' && rowItems.length) focus = 'row:0';
    return;
  }
  if (focus.startsWith('row:')) {
    const i = +focus.slice(4);
    if (dir === 'left' && i > 0) focus = 'row:' + (i - 1);
    else if (dir === 'right' && i < rowItems.length - 1) focus = 'row:' + (i + 1);
    else if (dir === 'up') focus = 'bar';
  }
}
function movePanel(dir) {
  if (focus === 'panelClose') {
    if (dir === 'down') focus = panel === 'quality' ? 'opt:quality:0' : 'opt:audio:0';
    return;
  }
  const [, kind, n] = focus.split(':');
  const i = +n;
  const count = optCount(kind);
  if (dir === 'up') focus = i > 0 ? `opt:${kind}:${i - 1}` : 'panelClose';
  else if (dir === 'down' && i < count - 1) focus = `opt:${kind}:${i + 1}`;
  else if (dir === 'right' && kind === 'audio' && optCount('sub'))
    focus = `opt:sub:${Math.min(i, optCount('sub') - 1)}`;
  else if (dir === 'left' && kind === 'sub')
    focus = `opt:audio:${Math.min(i, optCount('audio') - 1)}`;
  scrollToFocus();
}
function activate() {
  if (focus === 'close') return leave('close');
  if (focus === 'panelClose') return closePanel();
  if (focus === 'bar') return toggle();
  if (focus.startsWith('row:')) {
    const item = rowItems[+focus.slice(4)];
    if (item) item.act();
    return;
  }
  if (focus.startsWith('opt:')) {
    const [, kind, n] = focus.split(':');
    chooseOption(kind, +n);
  }
}
document.addEventListener('keydown', (e) => {
  plog(`key ${e.keyCode}`, music ? '(music)' : `focus ${focus}${panel ? ` panel ${panel}` : ''}`);
  if (music) {
    musicKey(e);
    return;
  }
  const hidden = !document.body.classList.contains('controls');
  const dirs = { 37: 'left', 38: 'up', 39: 'right', 40: 'down' };
  const k = e.keyCode;
  if (k === 461 || k === 27 || k === 8) {
    // Back
    e.preventDefault();
    if (panel) closePanel();
    else leave('back ' + k);
    return;
  }
  if (k === 415) {
    video.play();
    reveal();
    return;
  } // Play
  if (k === 19) {
    video.pause();
    mark();
    reveal();
    return;
  } // Pause
  if (k === 417) {
    seekBy(30);
    return;
  } // Fast-forward
  if (k === 412) {
    seekBy(-30);
    return;
  } // Rewind
  if (k === 413) {
    leave('stop');
    return;
  } // Stop
  if (hidden && !panel) {
    // The first press shows the controls; left/right already seek.
    if (k === 37 || k === 39) {
      focus = 'bar';
      move(dirs[k]);
      reveal();
      render();
    } else if (k === 13) {
      toggle();
      render();
    } else {
      reveal();
      render();
    }
    return;
  }
  if (dirs[k]) {
    e.preventDefault();
    move(dirs[k]);
  } else if (k === 13) activate();
  reveal();
  render();
});

// ---------- remote: pointer ----------
function barSec(x) {
  const d = duration();
  return Math.max(0, Math.min(1, (x - PAD) / WIDE)) * d;
}
function onBar(x, y) {
  return y >= 816 && y <= 886 && x >= PAD - 20 && x <= PAD + WIDE + 20;
}
document.addEventListener('mousemove', (e) => {
  if (music) {
    if (scrubbing) {
      pendingSeek = musicBarSec(e.clientX);
      renderMusic();
      return;
    }
    const t = e.target.closest && e.target.closest('[data-focus]');
    if (t) {
      focus = t.dataset.focus;
      renderMusic();
    }
    return;
  }
  reveal();
  if (scrubbing) {
    hoverSec = barSec(e.clientX);
    pendingSeek = hoverSec;
    render();
    return;
  }
  hoverSec = !panel && onBar(e.clientX, e.clientY) ? barSec(e.clientX) : null;
  const t = e.target.closest && e.target.closest('[data-focus]');
  if (t) focus = t.dataset.focus;
  else if (hoverSec !== null) focus = 'bar';
  render();
});
document.addEventListener('mousedown', (e) => {
  if (music) {
    if (e.target.closest && e.target.closest('#mBar')) {
      scrubbing = true;
      clearTimeout(seekTimer);
      pendingSeek = musicBarSec(e.clientX);
      focus = 'm:bar';
      renderMusic();
    }
    return;
  }
  reveal();
  if (!panel && onBar(e.clientX, e.clientY)) {
    scrubbing = true;
    clearTimeout(seekTimer);
    hoverSec = pendingSeek = barSec(e.clientX);
    focus = 'bar';
    render();
  }
});
document.addEventListener('mouseup', (e) => {
  if (music) {
    if (scrubbing) {
      scrubbing = false;
      seekTo(musicBarSec(e.clientX), true);
      return;
    }
    const t = e.target.closest && e.target.closest('[data-focus]');
    if (t) {
      focus = t.dataset.focus;
      musicActivate();
    }
    renderMusic();
    return;
  }
  if (scrubbing) {
    scrubbing = false;
    seekTo(barSec(e.clientX), true);
    return;
  }
  const t = e.target.closest && e.target.closest('[data-focus]');
  if (t) {
    focus = t.dataset.focus;
    activate();
    render();
    return;
  }
  if (panel && !e.target.closest('#panel')) {
    closePanel();
    return;
  }
  if (!panel && e.clientY > 250 && e.clientY < 770) toggle();
  render();
});
document.addEventListener(
  'wheel',
  (e) => {
    if (music) {
      music.first = Math.max(
        0,
        Math.min(music.songs.length - SONGS_SHOWN, music.first + (e.deltaY > 0 ? 1 : -1)),
      );
      renderMusic();
      return;
    }
    reveal();
    if (panel) {
      move(e.deltaY > 0 ? 'down' : 'up');
      render();
      return;
    }
    seekBy(e.deltaY > 0 ? -SEEK_STEP : SEEK_STEP);
  },
  { passive: true },
);

// ---------- music ----------
// An album from Jellyfin: the "now playing" screen replaces the video layer.
// Focus: 'm:bar' | 'm:prev' | 'm:play' | 'm:next' | 'm:shuffle' | 'song:N' | 'm:close'.
const SONG_H = 70;
const SONGS_SHOWN = 12;
let music = null; // {songs, order, at, shuffle, first}
const M_BUTTONS = ['m:prev', 'm:play', 'm:next', 'm:shuffle'];
function shuffled(n, first) {
  const rest = [];
  for (let i = 0; i < n; i++) if (i !== first) rest.push(i);
  for (let i = rest.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [rest[i], rest[j]] = [rest[j], rest[i]];
  }
  return [first].concat(rest);
}
function plainOrder(n) {
  const o = [];
  for (let i = 0; i < n; i++) o.push(i);
  return o;
}
function startMusic() {
  const m = p.music;
  const n = m.songs.length;
  const first = Math.max(0, Math.min(n - 1, m.start || 0));
  const pick = m.shuffle ? Math.floor(Math.random() * n) : first;
  music = {
    songs: m.songs,
    shuffle: !!m.shuffle,
    order: m.shuffle ? shuffled(n, pick) : plainOrder(n),
    at: 0,
    first: 0,
  };
  music.at = music.order.indexOf(pick);
  document.body.classList.add('music', 'controls');
  $('music').classList.remove('hidden');
  const cover = m.cover || '';
  [$('mBg'), $('mCover')].forEach((img) => {
    img.onerror = () => {
      img.removeAttribute('src');
      img.classList.add('empty');
    };
    img.classList.toggle('empty', !cover);
    if (cover) img.src = cover;
    else img.removeAttribute('src');
  });
  $('mClose').innerHTML = icon('close');
  $('mClose').dataset.focus = 'm:close';
  $('mBar').dataset.focus = 'm:bar';
  $('mListTitle').textContent = `SONGS · ${n}`;
  buildSongs();
  focus = 'm:play';
  playSong(music.order[music.at], false);
}
function current() {
  return music.order[music.at];
}
function playSong(i, fresh) {
  const song = music.songs[i];
  if (!song) return;
  plog(`song ${i + 1} of ${music.songs.length}`);
  if (p.report) {
    if (fresh !== false && playing) report('stop');
    p.report.item = song.item;
    p.report.source = song.item;
  }
  p.durationMs = song.ms || 0;
  music.at = Math.max(0, music.order.indexOf(i));
  $('mSong').textContent = song.title || '';
  $('mArtist').textContent = [song.artist || p.music.artist, p.music.album]
    .filter(Boolean)
    .join(' · ');
  keepSong(i);
  buildButtons();
  open(song.url, 0);
}
function step(delta) {
  // Back past three seconds restarts the song, as players do.
  if (delta < 0 && video.currentTime > 3) {
    seekTo(0, true);
    return;
  }
  const at = music.at + delta;
  if (at < 0 || at >= music.order.length) return;
  music.at = at;
  playSong(music.order[at]);
}
function toggleShuffle() {
  const now = current();
  music.shuffle = !music.shuffle;
  music.order = music.shuffle ? shuffled(music.songs.length, now) : plainOrder(music.songs.length);
  music.at = music.order.indexOf(now);
  buildButtons();
}
function buildButtons() {
  const box = $('mButtons');
  box.innerHTML = '';
  const defs = [
    ['m:prev', 'skip-back'],
    ['m:play', video.paused && playing ? 'play' : 'pause'],
    ['m:next', 'skip-forward'],
    ['m:shuffle', 'shuffle'],
  ];
  defs.forEach(([key, name], i) => {
    const b = el(
      'div',
      'btn round' + (key === 'm:shuffle' && music.shuffle ? ' on' : ''),
      icon(name),
    );
    b.dataset.focus = key;
    b.style.left = 140 + i * 104 + 'px';
    box.appendChild(b);
  });
}
function buildSongs() {
  const list = $('mList');
  list.innerHTML = '';
  music.songs.forEach((s, i) => {
    const row = el(
      'div',
      'song',
      `<span class="n">${i + 1}</span><span class="t">${esc(s.title || '')}</span><span class="d">${s.ms ? clock(s.ms / 1000) : ''}</span>`,
    );
    row.dataset.focus = 'song:' + i;
    list.appendChild(row);
  });
  placeSongs();
}
function placeSongs() {
  const rows = $('mList').children;
  for (let i = 0; i < rows.length; i++) {
    rows[i].style.top = (i - music.first) * SONG_H + 'px';
    const on = i === current();
    rows[i].classList.toggle('playing', on);
    rows[i].querySelector('.n').innerHTML = on
      ? icon(video.paused && playing ? 'pause' : 'play')
      : String(i + 1);
  }
}
function keepSong(i) {
  if (i < music.first) music.first = i;
  else if (i >= music.first + SONGS_SHOWN) music.first = i + 1 - SONGS_SHOWN;
  music.first = Math.max(0, Math.min(music.first, music.songs.length - SONGS_SHOWN));
}
function renderMusic() {
  if (!music) return;
  const d = duration(),
    t = now();
  const share = d ? Math.min(1, Math.max(0, t / d)) : 0;
  $('mFilled').style.width = share * 600 + 'px';
  $('mThumb').style.left = share * 600 + 'px';
  $('mPos').textContent = clock(t);
  $('mRemain').textContent = d ? '−' + clock(d - t) : '';
  const play = document.querySelector('[data-focus="m:play"]');
  const want = icon(video.paused && playing ? 'play' : 'pause');
  if (play && play.dataset.icon !== want.length + '') {
    play.innerHTML = want;
    play.dataset.icon = want.length + '';
  }
  placeSongs();
  paintFocus();
}
function musicMove(dir) {
  if (focus === 'm:close') {
    if (dir === 'down' || dir === 'left') focus = 'm:bar';
    return;
  }
  if (focus === 'm:bar') {
    if (dir === 'left') seekBy(-SEEK_STEP);
    else if (dir === 'right') seekBy(SEEK_STEP);
    else if (dir === 'up') focus = 'm:close';
    else if (dir === 'down') focus = 'm:play';
    return;
  }
  const b = M_BUTTONS.indexOf(focus);
  if (b >= 0) {
    if (dir === 'left' && b > 0) focus = M_BUTTONS[b - 1];
    else if (dir === 'right' && b < M_BUTTONS.length - 1) focus = M_BUTTONS[b + 1];
    else if (dir === 'right') {
      focus = 'song:' + current();
      keepSong(current());
    } else if (dir === 'up') focus = 'm:bar';
    return;
  }
  if (focus.startsWith('song:')) {
    const i = +focus.slice(5);
    if (dir === 'up' && i > 0) focus = 'song:' + (i - 1);
    else if (dir === 'up') focus = 'm:close';
    else if (dir === 'down' && i < music.songs.length - 1) focus = 'song:' + (i + 1);
    else if (dir === 'left') focus = 'm:shuffle';
    if (focus.startsWith('song:')) keepSong(+focus.slice(5));
  }
}
function musicActivate() {
  if (focus === 'm:close') return leave('close');
  if (focus === 'm:bar' || focus === 'm:play') return toggle();
  if (focus === 'm:prev') return step(-1);
  if (focus === 'm:next') return step(1);
  if (focus === 'm:shuffle') return toggleShuffle();
  if (focus.startsWith('song:')) {
    const i = +focus.slice(5);
    if (i === current()) toggle();
    else playSong(i);
  }
}
function musicKey(e) {
  const k = e.keyCode;
  const dirs = { 37: 'left', 38: 'up', 39: 'right', 40: 'down' };
  if (k === 461 || k === 27 || k === 8) {
    e.preventDefault();
    leave('back ' + k);
    return;
  }
  if (k === 415) video.play();
  else if (k === 19) {
    video.pause();
    mark();
  } else if (k === 417) seekBy(30);
  else if (k === 412) seekBy(-30);
  else if (k === 413) {
    leave('stop');
    return;
  } else if (dirs[k]) {
    e.preventDefault();
    musicMove(dirs[k]);
  } else if (k === 13) musicActivate();
  renderMusic();
}
function musicBarSec(x) {
  const d = duration();
  return Math.max(0, Math.min(1, (x - 140) / 600)) * d;
}

// ---------- start ----------
function params() {
  try {
    return JSON.parse((window.PalmSystem && PalmSystem.launchParams) || '{}') || {};
  } catch {
    return {};
  }
}
function start() {
  const next = params();
  if (window.LumaLog) window.LumaLog.configure(next.debugLog);
  if (!next.url) {
    plog('launched without a stream');
    return;
  }
  plog(
    'start:',
    next.music
      ? 'music'
      : next.jellyfin
        ? 'jellyfin'
        : next.report
          ? next.report.service || 'media'
          : 'url',
    next.title ? `"${next.title}"` : '',
    next.resumeMs ? `resume at ${clock(next.resumeMs / 1000)}` : '',
    `· player ${PLAYER_VERSION} · ${navigator.userAgent}`,
  );
  leaving = false;
  p = next;
  p.qualities = p.qualities || [];
  qualityIndex = Math.max(0, p.quality || 0);
  audioIndex = 0;
  subtitleIndex = -1;
  cues = [];
  panel = null;
  focus = 'bar';
  $('tracks').classList.add('hidden');
  applyAccent();
  music = null;
  document.body.classList.remove('music');
  $('music').classList.add('hidden');
  if (p.music && (p.music.songs || []).length) {
    startMusic();
    render();
    renderMusic();
    return;
  }
  jf = null;
  if (p.jellyfin && p.jellyfin.source && window.LumaStreams) {
    jfStart((p.resumeMs || 0) / 1000);
    reveal();
    render();
    return;
  }
  header();
  buildRow();
  if (typeof p.subtitle === 'number' && p.subtitle >= 0) setSubtitle(p.subtitle);
  open(p.url, (p.resumeMs || 0) / 1000);
  reveal();
  render();
}
// With handlesRelaunch, webOS leaves a relaunched app where it is: a player
// left running in the background (Home pressed during a film) must bring
// itself to the front, or it opens the next film hidden and video never starts.
function bringToFront() {
  try {
    const system = window.webOSSystem || window.PalmSystem;
    if (system && typeof system.activate === 'function') system.activate();
  } catch (e) {
    plog(`could not come to the front: ${e && e.message}`);
  }
}
document.addEventListener('webOSRelaunch', () => {
  plog('relaunched: coming to the front');
  bringToFront();
  start();
});
document.addEventListener('visibilitychange', () => {
  if (document.hidden) {
    mark();
    video.pause();
  }
});
loadIcons(
  [
    'close',
    'volume',
    'captions',
    'quality',
    'list',
    'skip-back',
    'skip-forward',
    'refresh',
    'check',
    'play',
    'pause',
    'shuffle',
  ],
  () => {
    start();
    setInterval(tick, 250);
  },
);
