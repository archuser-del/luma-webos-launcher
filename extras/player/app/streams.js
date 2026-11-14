// How Luma Player plays a Jellyfin file. The TV is asked what it can decode
// (canPlayType), and the file is played as it is whenever possible:
//
//   Direct Play    the original file (MKV and all), untouched.
//   Direct Stream  the original video, copied bit for bit (HDR intact), in a
//                  new container; only audio the TV cannot decode is
//                  converted, to FLAC (lossless) when the TV plays FLAC.
//   Transcoding    only when the TV cannot decode the video codec, when an
//                  image subtitle (PGS, VobSub) has to be burned in, or when
//                  the viewer picked a lower quality.
//
// Jellyfin tone-maps HDR to SDR whenever it re-encodes HDR video, so HDR
// video is never re-encoded unless the viewer explicitly picks an image
// subtitle that is labelled as burned in and SDR.
//
// Pure functions: player.js and the Node tests share this file.
(function (root) {
  'use strict';

  const CONTAINERS = ['mkv', 'matroska', 'webm', 'mp4', 'm4v', 'mov', 'ts', 'mpegts', 'm2ts'];
  const TEXT_SUBTITLES = [
    'srt',
    'subrip',
    'ass',
    'ssa',
    'vtt',
    'webvtt',
    'mov_text',
    'ttml',
    'text',
    'smi',
    'sami',
  ];
  // What the TV is asked about each codec; any "maybe" or "probably" counts.
  const AUDIO_PROBES = {
    aac: ['audio/mp4; codecs="mp4a.40.2"', 'audio/aac'],
    mp3: ['audio/mpeg', 'audio/mp4; codecs="mp4a.40.34"'],
    mp2: ['audio/mp4; codecs="mp4a.6B"'],
    ac3: ['audio/mp4; codecs="ac-3"', 'audio/ac3'],
    eac3: ['audio/mp4; codecs="ec-3"', 'audio/eac3'],
    flac: ['audio/mp4; codecs="flac"', 'audio/flac'],
    alac: ['audio/mp4; codecs="alac"'],
    opus: ['audio/mp4; codecs="opus"', 'audio/webm; codecs="opus"'],
    vorbis: ['audio/webm; codecs="vorbis"', 'audio/ogg; codecs="vorbis"'],
    // webOS rarely says yes to these even where it plays them (Jellyfin's
    // client has manual switches for both); Luma has them in Settings › System.
    dts: [
      'audio/mp4; codecs="dts-"',
      'audio/mp4; codecs="dts+"',
      'audio/mp4; codecs="dtsc"',
      'video/mp4; codecs="dts-"',
      'audio/vnd.dts',
      'audio/x-dts',
    ],
    truehd: [
      'audio/mp4; codecs="mlpa"',
      'video/mp4; codecs="mlpa"',
      'audio/true-hd',
      'audio/x-truehd',
      'audio/vnd.dolby.mlp',
    ],
    pcm: ['audio/wav; codecs="1"', 'audio/wav'],
  };
  const VIDEO_PROBES = {
    h264: ['video/mp4; codecs="avc1.640033"', 'video/mp4; codecs="avc1.42E01E"'],
    hevc: ['video/mp4; codecs="hvc1.2.4.L153.B0"', 'video/mp4; codecs="hev1.1.6.L120.90"'],
    av1: ['video/mp4; codecs="av01.0.15M.10"'],
    vp9: ['video/webm; codecs="vp09.02.51.10"', 'video/webm; codecs="vp9"'],
    vp8: ['video/webm; codecs="vp8"'],
    mpeg2video: ['video/mp2t; codecs="mp2v"', 'video/mpeg'],
    vc1: ['video/x-ms-wmv', 'video/mp4; codecs="vc-1"'],
    mpeg4: ['video/mp4; codecs="mp4v.20.8"'],
  };
  // Lower qualities offered for SDR files: re-encoded to H.264.
  const LADDER = [
    { name: '1080p · 20 Mbps', bitrate: 20000000, height: 1080 },
    { name: '1080p · 10 Mbps', bitrate: 10000000, height: 1080 },
    { name: '720p · 6 Mbps', bitrate: 6000000, height: 720 },
    { name: '720p · 3 Mbps', bitrate: 3000000, height: 720 },
    { name: '480p · 1.5 Mbps', bitrate: 1500000, height: 480 },
  ];
  const CODEC_NAMES = {
    truehd: 'TrueHD',
    dts: 'DTS',
    eac3: 'E-AC-3',
    ac3: 'AC-3',
    flac: 'FLAC',
    aac: 'AAC',
    opus: 'Opus',
    vorbis: 'Vorbis',
    pcm: 'PCM',
    alac: 'ALAC',
    mp3: 'MP3',
    mp2: 'MP2',
    hevc: 'HEVC',
    h264: 'H.264',
    av1: 'AV1',
    vp9: 'VP9',
    mpeg2video: 'MPEG-2',
    vc1: 'VC-1',
  };

  function lower(v) {
    return String(v || '').toLowerCase();
  }
  // "pcm_s24le" and "dca" are families the probe table knows by one name.
  function family(codec) {
    const c = lower(codec);
    if (c.indexOf('pcm') === 0) return 'pcm';
    if (c === 'dca' || c.indexOf('dts') === 0) return 'dts';
    if (c === 'mlp') return 'truehd';
    if (c === 'h265') return 'hevc';
    if (c === 'avc') return 'h264';
    if (c === 'mpeg2') return 'mpeg2video';
    if (c === 'wmv3') return 'vc1';
    return c;
  }
  function codecName(codec) {
    const f = family(codec);
    return CODEC_NAMES[f] || f.toUpperCase();
  }

  /** What this TV can decode, asked of a <video> element. */
  // `settings`: {truehd, dts} as set in Luma: 'auto' asks the TV, 'on' and
  // 'off' overrule its answer.
  function capabilities(el, settings) {
    const ask = (types) =>
      types.some((t) => {
        try {
          return !!el.canPlayType(t);
        } catch {
          return false;
        }
      });
    const audio = {};
    const video = {};
    Object.keys(AUDIO_PROBES).forEach((k) => (audio[k] = ask(AUDIO_PROBES[k])));
    Object.keys(VIDEO_PROBES).forEach((k) => (video[k] = ask(VIDEO_PROBES[k])));
    // Every webOS TV decodes these; a cautious probe never turns them off.
    audio.aac = true;
    video.h264 = true;
    const asked = { truehd: audio.truehd, dts: audio.dts };
    ['truehd', 'dts'].forEach((k) => {
      const set = settings && settings[k];
      if (set === 'on') audio[k] = true;
      if (set === 'off') audio[k] = false;
    });
    return { audio, video, asked };
  }

  // webOS plays FLAC in video only as stereo (Jellyfin's own webOS profile
  // has the same rule); surround FLAC is refused like an unknown codec.
  function canAudio(caps, codec, channels) {
    if (family(codec) === 'flac' && channels > 2) return false;
    return !!caps.audio[family(codec)];
  }
  function canVideo(caps, codec) {
    return !!caps.video[family(codec)];
  }

  /** The parts of a Jellyfin MediaSource that matter for playback. */
  function sourceInfo(source) {
    const streams = (source && source.MediaStreams) || [];
    const video = streams.find((s) => s.Type === 'Video') || {};
    const range = video.VideoRangeType || video.VideoRange || 'SDR';
    const audio = streams
      .filter((s) => s.Type === 'Audio')
      .map((s, position) => ({
        index: s.Index,
        position,
        codec: lower(s.Codec),
        name: s.DisplayTitle || s.Title || s.Language || `Track ${position + 1}`,
        isDefault: !!s.IsDefault,
        channels: s.Channels || 0,
        external: !!s.IsExternal,
      }));
    const subtitles = streams
      .filter((s) => s.Type === 'Subtitle')
      .map((s) => {
        const codec = lower(s.Codec);
        const text = s.IsTextSubtitleStream === true || TEXT_SUBTITLES.indexOf(codec) >= 0;
        return {
          index: s.Index,
          codec,
          name: s.DisplayTitle || s.Title || s.Language || 'Subtitles',
          image: !text,
          isDefault: !!s.IsDefault,
          forced: !!s.IsForced,
        };
      });
    return {
      id: source && source.Id,
      name: (source && source.Name) || '',
      container: lower(source && source.Container),
      bitrate: (source && source.Bitrate) || video.BitRate || 0,
      video: {
        codec: lower(video.Codec),
        range,
        hdr: lower(video.VideoRange) === 'hdr' || (range !== 'SDR' && range !== 'Unknown'),
        height: video.Height || 0,
        width: video.Width || 0,
      },
      audio,
      subtitles,
    };
  }

  /** Lower qualities to offer. None for HDR: re-encoding would make it SDR. */
  function qualities(info) {
    if (info.video.hdr) return [];
    const h = info.video.height || 1080;
    const b = info.bitrate || Infinity;
    return LADDER.filter((q) => q.height <= Math.max(h, 480) && q.bitrate < b);
  }

  /** The label a subtitle gets in pickers; image subtitles say what they cost. */
  function subtitleLabel(info, sub) {
    if (!sub.image) return sub.name;
    return info.video.hdr ? `${sub.name} · burned in, SDR` : `${sub.name} · burned in`;
  }

  function containerOk(info) {
    return lower(info.container)
      .split(',')
      .some((c) => CONTAINERS.indexOf(c.trim()) >= 0);
  }

  function query(pairs) {
    return Object.keys(pairs)
      .filter((k) => pairs[k] !== undefined && pairs[k] !== null && pairs[k] !== '')
      .map((k) => `${k}=${encodeURIComponent(pairs[k])}`)
      .join('&');
  }

  /**
   * Decide how to play. ctx = {base, token, deviceId, item, session, info,
   * caps, audio (stream index or null), subtitle (stream index or null),
   * quality ({bitrate, height}) or null, forceStream (bool)}.
   * Returns {url, method, reasons[], audioOut, audioPosition, burn, label}.
   */
  function plan(ctx) {
    const info = ctx.info;
    const caps = ctx.caps;
    const track =
      info.audio.find((a) => a.index === ctx.audio) ||
      info.audio.find((a) => a.isDefault) ||
      info.audio[0] ||
      null;
    const sub =
      ctx.subtitle === null || ctx.subtitle === undefined
        ? null
        : info.subtitles.find((s) => s.index === ctx.subtitle) || null;
    const burn = !!(sub && sub.image);
    // A lower quality is never applied to HDR: it would be re-encoded to SDR.
    const quality = info.video.hdr ? null : ctx.quality;
    const reasons = [];
    if (info.video.codec && !canVideo(caps, info.video.codec)) {
      reasons.push(`${codecName(info.video.codec)} video not supported`);
    }
    if (burn) reasons.push('burning in subtitles');
    if (quality) reasons.push(`lower quality (${Math.round(quality.bitrate / 1e6)} Mbps)`);
    const transcodeVideo = reasons.length > 0;
    const audioOk = !track || canAudio(caps, track.codec, track.channels);
    const common = {
      MediaSourceId: info.id,
      PlaySessionId: ctx.session,
      DeviceId: ctx.deviceId,
      ApiKey: ctx.token,
    };

    if (!transcodeVideo && audioOk && containerOk(info) && !ctx.forceStream) {
      const ext = lower(info.container).split(',')[0] || 'mkv';
      return {
        url: `${ctx.base}/Videos/${ctx.item}/stream.${ext}?${query(Object.assign({ static: 'true' }, common))}`,
        method: 'DirectPlay',
        reasons,
        audioOut: track ? track.codec : '',
        audioPosition: track ? track.position : 0,
        burn: false,
        label: label('Direct Play', info, track, null),
      };
    }

    // HLS (fMP4) from Jellyfin, the way Jellyfin's own webOS app streams:
    // audio the TV decodes is copied; anything else is converted. A stereo
    // track may become FLAC (lossless); surround becomes E-AC-3 5.1, as webOS
    // refuses FLAC with more than two channels. ctx.fallback steps down to
    // AC-3, then AAC, when the TV refuses a stream.
    const channels = track ? track.channels : 0;
    const dolby = ['eac3', 'ac3', 'aac']
      .filter((c) => caps.audio[c])
      .slice(Math.min(ctx.fallback || 0, 2));
    const targets = (
      caps.audio.flac && channels > 0 && channels <= 2 && !ctx.fallback ? ['flac'] : []
    ).concat(dolby.length ? dolby : ['aac']);
    const audioCodecs = audioOk && track ? [family(track.codec)].concat(targets) : targets;
    const audioOut = audioOk && track ? track.codec : audioCodecs[0];
    const pairs = Object.assign({}, common, {
      SegmentContainer: 'mp4',
      AudioStreamIndex: track ? track.index : undefined,
      AudioCodec: audioCodecs.filter((c, i) => audioCodecs.indexOf(c) === i).join(','),
      // Jellyfin's Dolby and AAC encoders stop at 5.1, FLAC here at stereo.
      TranscodingMaxAudioChannels: audioOk ? 8 : audioOut === 'flac' ? 2 : 6,
      BreakOnNonKeyFrames: 'true',
    });
    if (!transcodeVideo) {
      // Asking for the file's own codec with no size, bitrate or range
      // limits makes Jellyfin copy the video stream: HDR stays HDR.
      pairs.VideoCodec = family(info.video.codec);
      pairs.AllowVideoStreamCopy = 'true';
      // The HDR kinds the TV takes. Dual-layer Dolby Vision (profile 7, an
      // enhancement layer) is left out: Jellyfin then strips the Dolby Vision
      // layers and copies the HDR10 base instead of sending a stream the TV
      // cannot start. Other Dolby Vision (LG TVs play it) is copied as is.
      if (['hevc', 'av1'].includes(pairs.VideoCodec)) {
        pairs[`${pairs.VideoCodec}-rangetype`] =
          'SDR,HDR10,HDR10Plus,HLG,DOVI,DOVIWithHDR10,DOVIWithHLG,DOVIWithSDR';
      }
    } else {
      pairs.VideoCodec = caps.video.hevc && info.video.hdr ? 'hevc,h264' : 'h264';
      pairs.AllowVideoStreamCopy = 'false';
      if (quality) {
        pairs.VideoBitrate = quality.bitrate;
        pairs.MaxStreamingBitrate = quality.bitrate + 640000;
        pairs.MaxHeight = quality.height;
      }
      if (burn) {
        pairs.SubtitleStreamIndex = sub.index;
        pairs.SubtitleMethod = 'Encode';
      }
    }
    const method = transcodeVideo ? 'Transcode' : 'DirectStream';
    return {
      // main.m3u8, not master.m3u8: for HDR video that is copied, Jellyfin's
      // master playlist also lists H.264 and HEVC SDR conversions at the same
      // bandwidth "for compatibility", and the TV may pick one of those (a
      // 416x234 re-encode of a 4K film). The variant playlist has only the
      // stream asked for.
      url: `${ctx.base}/Videos/${ctx.item}/main.m3u8?${query(pairs)}`,
      method,
      reasons,
      audioOut,
      // A converted or selected HLS rendition has just the one audio track.
      audioPosition: 0,
      burn,
      // Another, plainer audio codec is left to try when the TV refuses.
      fallbackLeft: !audioOk && (ctx.fallback || 0) < 2,
      label: label(
        method === 'DirectStream' ? 'Direct Stream' : 'Transcoding',
        info,
        track,
        audioOk ? null : audioOut,
        transcodeVideo,
      ),
    };
  }

  /** "Direct Play · HDR10 · TrueHD → FLAC" for the player's info line. */
  function label(method, info, track, convertedTo, videoReencoded) {
    const parts = [method];
    if (info.video.hdr) {
      parts.push(
        videoReencoded ? 'SDR (converted)' : info.video.range.replace('DOVI', 'Dolby Vision'),
      );
    }
    if (track && convertedTo) parts.push(`${codecName(track.codec)} → ${codecName(convertedTo)}`);
    else if (track) parts.push(codecName(track.codec));
    return parts.join(' · ');
  }

  /** WebVTT address of a text subtitle; image subtitles have none. */
  function subtitleUrl(ctx, sub) {
    if (!sub || sub.image) return null;
    return `${ctx.base}/Videos/${ctx.item}/${ctx.info.id}/Subtitles/${sub.index}/0/Stream.vtt?ApiKey=${encodeURIComponent(ctx.token)}`;
  }

  const api = {
    capabilities,
    sourceInfo,
    qualities,
    subtitleLabel,
    subtitleUrl,
    plan,
    codecName,
    family,
  };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.LumaStreams = api;
})(typeof window !== 'undefined' ? window : this);
