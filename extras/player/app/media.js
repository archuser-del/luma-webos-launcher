// Audio track choice and HDR detection for the Luma player.
// Pure functions with no DOM access: the player loads this file as a script,
// and tests/player-media.test.mjs runs it in Node.
'use strict';

(function (root) {
  // Codecs Jellyfin can copy into the fMP4 HLS segments the player asks for.
  // TrueHD, DTS and PCM are re-encoded there whatever the TV supports.
  const HLS_COPY = ['eac3', 'ac3', 'aac', 'mp3', 'flac', 'opus'];
  // Codecs the player asks the server to re-encode into, best first. The
  // player only lists the ones the TV reports it can play.
  const TARGETS = ['eac3', 'ac3', 'aac', 'mp3', 'flac', 'opus'];
  // Media types the TV is asked about, per codec. Any "yes" counts.
  const MIME = {
    aac: ['audio/mp4; codecs="mp4a.40.2"', 'video/mp4; codecs="avc1.640028, mp4a.40.2"'],
    ac3: ['audio/mp4; codecs="ac-3"', 'video/mp4; codecs="avc1.640028, ac-3"'],
    eac3: ['audio/mp4; codecs="ec-3"', 'video/mp4; codecs="avc1.640028, ec-3"'],
    mp3: ['audio/mpeg', 'audio/mp4; codecs="mp4a.69"'],
    opus: ['audio/mp4; codecs="opus"', 'audio/webm; codecs="opus"'],
    flac: ['audio/mp4; codecs="flac"', 'audio/flac'],
    dts: ['audio/mp4; codecs="dtsc"', 'audio/mp4; codecs="dtsh"', 'audio/vnd.dts'],
    truehd: ['audio/mp4; codecs="mlpa"'],
    vorbis: ['audio/webm; codecs="vorbis"', 'audio/ogg; codecs="vorbis"'],
    alac: ['audio/mp4; codecs="alac"'],
  };

  function lower(v) {
    return String(v === undefined || v === null ? '' : v).toLowerCase();
  }

  function codecOf(raw) {
    const c = lower(raw);
    if (c === 'dca' || c.indexOf('dca-') === 0 || c.indexOf('dts') === 0) return 'dts';
    if (c.indexOf('pcm') === 0) return 'pcm';
    if (c === 'mlp') return 'truehd';
    if (c === 'ec-3' || c === 'e-ac-3') return 'eac3';
    if (c === 'ac-3') return 'ac3';
    return c;
  }

  // Source quality: lossless first, then channels, then codec, then bitrate.
  function grade(codec, detail) {
    const atmos = /atmos/.test(detail);
    const dtsx = /dts[:-]?x\b/.test(detail);
    switch (codec) {
      case 'truehd':
        return { lossless: true, tier: atmos ? 10 : 9 };
      case 'dts':
        if (dtsx) return { lossless: true, tier: 10 };
        if (/master audio|dts-hd ma|dca-ma|(^|[\s(])ma([\s)]|$)/.test(detail))
          return { lossless: true, tier: 9 };
        if (/hra|high resolution|dca-hra/.test(detail)) return { lossless: false, tier: 6 };
        return { lossless: false, tier: 5 };
      case 'flac':
      case 'pcm':
      case 'alac':
        return { lossless: true, tier: 8 };
      case 'eac3':
        return { lossless: false, tier: atmos ? 7 : 6 };
      case 'ac3':
      case 'opus':
        return { lossless: false, tier: 4 };
      case 'aac':
      case 'vorbis':
        return { lossless: false, tier: 3 };
      case 'mp3':
        return { lossless: false, tier: 2 };
      case 'mp2':
        return { lossless: false, tier: 1 };
      default:
        return { lossless: false, tier: 0 };
    }
  }

  // Commentary and audio description are never picked on their own.
  const EXTRA =
    /commentary|director'?s comments?|audio description|descriptive|described video|visually impaired|(^|[\s\-(])ad([\s)]|$)/;

  function track(fields, pos) {
    const detail = [fields.profile, fields.rawCodec, fields.spatial, fields.title]
      .map(lower)
      .join(' ');
    const codec = codecOf(fields.rawCodec);
    const g = grade(codec, detail);
    return {
      pos,
      index: fields.index,
      codec,
      channels: Math.max(0, Math.min(8, +fields.channels || 0)),
      bitrate: +fields.bitrate || 0,
      lang: lower(fields.lang),
      title: String(fields.title || fields.lang || `Track ${pos + 1}`),
      isDefault: !!fields.isDefault,
      lossless: g.lossless,
      tier: g.tier,
      extra: !!fields.extra || EXTRA.test(lower(fields.title)),
    };
  }

  // Jellyfin MediaStreams, in server order.
  function fromJellyfin(streams) {
    return (streams || [])
      .filter((s) => s && s.Type === 'Audio')
      .map((s, pos) =>
        track(
          {
            index: s.Index,
            rawCodec: s.Codec,
            profile: s.Profile,
            spatial: s.AudioSpatialFormat,
            channels: s.Channels,
            bitrate: s.BitRate,
            lang: s.Language,
            title: s.DisplayTitle || s.Title || s.Language,
            isDefault: s.IsDefault,
          },
          pos,
        ),
      );
  }

  // Plex Part.Stream entries (streamType 2 is audio). Plex bitrates are kbit/s.
  function fromPlex(streams) {
    return (streams || [])
      .filter((s) => s && +s.streamType === 2)
      .map((s, pos) =>
        track(
          {
            index: s.id,
            rawCodec: s.codec,
            profile: s.profile,
            channels: s.channels,
            bitrate: (+s.bitrate || 0) * 1000,
            lang: s.languageCode || s.languageTag || '',
            title: s.extendedDisplayTitle || s.displayTitle || s.title || s.language,
            isDefault: s.selected || s.default,
            extra: s.visualImpaired === true || s.visualImpaired === 1,
          },
          pos,
        ),
      );
  }

  function jellyfinHdr(streams) {
    return (streams || []).some((s) => {
      if (!s || s.Type !== 'Video') return false;
      const range = lower(s.VideoRange);
      const kind = lower(s.VideoRangeType);
      return (range && range !== 'sdr' && range !== 'unknown') || /hdr|dovi|hlg/.test(kind);
    });
  }

  function plexHdr(streams) {
    return (streams || []).some(
      (s) =>
        s &&
        +s.streamType === 1 &&
        (s.DOVIPresent === true ||
          s.DOVIPresent === 1 ||
          /smpte2084|arib-std-b67/.test(lower(s.colorTrc))),
    );
  }

  // {service, streams, hls, part} from Luma (or fetched for another episode).
  function describe(info) {
    const plex = info && info.service === 'plex';
    const streams = (info && info.streams) || [];
    return {
      service: plex ? 'plex' : 'jellyfin',
      hls: (info && info.hls) || '',
      part: info && info.part ? String(info.part) : '',
      tracks: plex ? fromPlex(streams) : fromJellyfin(streams),
      hdr: plex ? plexHdr(streams) : jellyfinHdr(streams),
    };
  }

  function isEnglish(t) {
    if (t.lang) return /^(en|eng)([-_].*)?$/.test(t.lang);
    return /\benglish\b/i.test(t.title);
  }

  function better(a, b) {
    return (
      b.lossless - a.lossless ||
      b.channels - a.channels ||
      b.tier - a.tier ||
      b.bitrate - a.bitrate ||
      a.pos - b.pos
    );
  }

  // Main English tracks by source quality; commentary and description last.
  function rank(tracks) {
    return tracks.slice().sort((a, b) => a.extra - b.extra || better(a, b));
  }

  // The first English track in the highest source quality. If the TV can't
  // play it as is, it is re-encoded to Dolby Digital Plus rather than falling
  // back to a lesser track (such as a Dolby Digital one). Only when the TV has
  // no DD+ either does a track it plays as is win.
  function pick(tracks, plays, eac3) {
    if (!tracks.length) return null;
    const english = tracks.filter(isEnglish);
    if (!english.length) return tracks.filter((t) => t.isDefault)[0] || tracks[0];
    const ranked = rank(english);
    const main = ranked.filter((t) => !t.extra);
    const pool = main.length ? main : ranked;
    const best = pool[0];
    if (plays(best) || eac3) return best;
    return pool.filter(plays)[0] || best;
  }

  // Codecs the server may copy or encode into, best target first. Every one
  // was checked on the TV first; AAC stays as the last resort.
  function audioCodecs(tvPlays) {
    const out = TARGETS.filter((c) => tvPlays(c));
    if (out.indexOf('aac') < 0) out.push('aac');
    return out;
  }

  // How to play: the file as is (direct) or HLS with the chosen track, copied
  // when the TV plays its codec and re-encoded otherwise.
  // opts: {direct: file URL in hand, switchable: the TV can switch tracks in
  // a file, tvPlays: codec => bool}.
  function plan(tracks, opts) {
    const codecs = audioCodecs(opts.tvPlays);
    const inFile = (t) =>
      !!opts.direct && opts.tvPlays(t.codec) && (t.pos === 0 || !!opts.switchable);
    const copy = (t) => HLS_COPY.indexOf(t.codec) >= 0 && codecs.indexOf(t.codec) >= 0;
    const t = pick(tracks, (x) => inFile(x) || copy(x), codecs[0] === 'eac3');
    if (!t) return null;
    if (inFile(t)) return { track: t, hls: false, convert: false, codecs };
    return { track: t, hls: true, convert: !copy(t), codecs };
  }

  // Sets one query parameter, leaving the others exactly as they were.
  function setQuery(url, key, value) {
    const v = encodeURIComponent(String(value));
    const re = new RegExp('([?&])' + key.replace(/[-.]/g, '\\$&') + '=[^&#]*');
    if (re.test(url)) return url.replace(re, (_, sep) => `${sep}${key}=${v}`);
    return url + (url.indexOf('?') >= 0 ? '&' : '?') + `${key}=${v}`;
  }

  function jellyfinUrl(hls, t, codecs) {
    return setQuery(setQuery(hls, 'AudioStreamIndex', t.index), 'AudioCodec', codecs.join(','));
  }

  // Plex chooses its encode targets from the client profile: Dolby codecs the
  // TV plays are added to it, DD+ first.
  function plexUrl(hls, codecs) {
    const extra = codecs
      .filter((c) => c === 'eac3' || c === 'ac3')
      .map(
        (c) =>
          `add-transcode-target-audio-codec(type=videoProfile&context=streaming&protocol=hls&audioCodec=${c})`,
      )
      .join('+');
    return extra ? setQuery(hls, 'X-Plex-Client-Profile-Extra', extra) : hls;
  }

  root.LumaMedia = {
    MIME,
    HLS_COPY,
    codecOf,
    fromJellyfin,
    fromPlex,
    describe,
    isEnglish,
    rank,
    pick,
    audioCodecs,
    plan,
    setQuery,
    jellyfinUrl,
    plexUrl,
  };
})(typeof window !== 'undefined' ? window : this);
