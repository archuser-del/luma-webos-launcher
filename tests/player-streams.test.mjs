// Luma Player's Jellyfin stream choice (extras/player/app/streams.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const file = path.resolve(import.meta.dirname, '../extras/player/app/streams.js');
const sandbox = { module: { exports: {} } };
vm.runInNewContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file });
const S = sandbox.module.exports;

// A 2021 LG OLED: HEVC/AV1, Dolby and FLAC, but no DTS or TrueHD.
const supported = new Set([
  'audio/mp4; codecs="ac-3"',
  'audio/mp4; codecs="ec-3"',
  'audio/mp4; codecs="flac"',
  'audio/mpeg',
  'video/mp4; codecs="hvc1.2.4.L153.B0"',
  'video/mp4; codecs="av01.0.15M.10"',
  'video/webm; codecs="vp09.02.51.10"',
]);
const tv = (extra = [], without = []) => ({
  canPlayType: (t) =>
    (supported.has(t) || extra.includes(t)) && !without.includes(t) ? 'probably' : '',
});

const stream = (Index, Type, Codec, more = {}) => ({ Index, Type, Codec, ...more });
const hdrRemux = {
  Id: 'src1',
  Container: 'mkv',
  Bitrate: 60000000,
  MediaStreams: [
    stream(0, 'Video', 'hevc', { VideoRange: 'HDR', VideoRangeType: 'HDR10', Height: 2160 }),
    stream(1, 'Audio', 'truehd', {
      DisplayTitle: 'English TrueHD 7.1',
      IsDefault: true,
      Channels: 8,
    }),
    stream(2, 'Audio', 'ac3', { DisplayTitle: 'English AC3 5.1', Channels: 6 }),
    stream(3, 'Subtitle', 'PGSSUB', { DisplayTitle: 'English PGS' }),
    stream(4, 'Subtitle', 'subrip', { DisplayTitle: 'English SRT', IsTextSubtitleStream: true }),
  ],
};
const sdrMovie = {
  Id: 'src2',
  Container: 'mkv',
  Bitrate: 12000000,
  MediaStreams: [
    stream(0, 'Video', 'h264', { VideoRange: 'SDR', VideoRangeType: 'SDR', Height: 1080 }),
    stream(1, 'Audio', 'eac3', { IsDefault: true, Channels: 6 }),
    stream(2, 'Subtitle', 'dvd_subtitle', { DisplayTitle: 'French' }),
  ],
};
const ctx = (source, caps, more = {}) => ({
  base: 'http://jf:8096',
  token: 'tok',
  deviceId: 'dev',
  item: 'item1',
  session: 'sess',
  info: S.sourceInfo(source),
  caps,
  audio: null,
  subtitle: null,
  quality: null,
  ...more,
});
const params = (url) => new URL(url).searchParams;

test('the TV is asked about each codec, and AAC/H.264 are always on', () => {
  const caps = S.capabilities(tv());
  assert.equal(caps.audio.flac, true);
  assert.equal(caps.audio.eac3, true);
  assert.equal(caps.audio.dts, false);
  assert.equal(caps.audio.truehd, false);
  assert.equal(caps.audio.aac, true);
  assert.equal(caps.video.hevc, true);
  assert.equal(caps.video.h264, true);
});

test('a supported MKV plays directly, original file and all', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(sdrMovie, caps));
  assert.equal(p.method, 'DirectPlay');
  assert.match(p.url, /\/Videos\/item1\/stream\.mkv\?static=true&MediaSourceId=src2/);
  assert.equal(params(p.url).get('api_key'), 'tok');
  assert.equal(p.label, 'Direct Play · E-AC-3');
});

test('HDR with TrueHD: the video is copied in HDR, only the audio becomes FLAC', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(hdrRemux, caps));
  assert.equal(p.method, 'DirectStream');
  const q = params(p.url);
  assert.equal(q.get('VideoCodec'), 'hevc');
  assert.equal(q.get('AllowVideoStreamCopy'), 'true');
  // No limits that would stop Jellyfin from copying the video.
  for (const key of [
    'VideoBitrate',
    'MaxHeight',
    'MaxWidth',
    'hevc-rangetype',
    'SubtitleStreamIndex',
  ]) {
    assert.equal(q.get(key), null, key);
  }
  assert.equal(q.get('AudioCodec').split(',')[0], 'flac');
  assert.equal(q.get('AudioStreamIndex'), '1');
  assert.equal(q.get('TranscodingMaxAudioChannels'), '8');
  assert.equal(p.audioOut, 'flac');
  assert.equal(p.label, 'Direct Stream · HDR10 · TrueHD → FLAC');
});

test('picking the AC3 track of the same HDR file plays it directly', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(hdrRemux, caps, { audio: 2 }));
  assert.equal(p.method, 'DirectPlay');
  assert.equal(p.audioPosition, 1);
  assert.equal(p.label, 'Direct Play · HDR10 · AC-3');
});

test('without FLAC on the TV, unsupported audio becomes Dolby instead', () => {
  const caps = S.capabilities(tv([], ['audio/mp4; codecs="flac"']));
  const p = S.plan(ctx(hdrRemux, caps));
  assert.equal(params(p.url).get('AudioCodec'), 'eac3,ac3,aac');
  assert.equal(params(p.url).get('TranscodingMaxAudioChannels'), '6');
  assert.match(p.label, /TrueHD → E-AC-3/);
});

test('a TV that decodes DTS gets DTS directly; supported audio is copied in HLS', () => {
  const dts = {
    ...sdrMovie,
    Container: 'avi',
    MediaStreams: [sdrMovie.MediaStreams[0], stream(1, 'Audio', 'dts', { IsDefault: true })],
  };
  const caps = S.capabilities(tv(['audio/mp4; codecs="dts-"']));
  const p = S.plan(ctx(dts, caps));
  // AVI is no direct-play container: remuxed into MKV, video and DTS copied.
  assert.equal(p.method, 'DirectStream');
  assert.match(new URL(p.url).pathname, /stream\.mkv$/);
  assert.equal(params(p.url).get('AudioCodec').split(',')[0], 'dts');
  assert.equal(params(p.url).get('VideoCodec'), 'h264');
});

test('text subtitles never transcode; image subtitles burn in', () => {
  const caps = S.capabilities(tv());
  const text = S.plan(ctx(hdrRemux, caps, { subtitle: 4, audio: 2 }));
  assert.equal(text.method, 'DirectPlay');
  const info = S.sourceInfo(hdrRemux);
  assert.match(
    S.subtitleUrl(ctx(hdrRemux, caps), info.subtitles[1]),
    /Subtitles\/4\/0\/Stream\.vtt/,
  );
  assert.equal(S.subtitleUrl(ctx(hdrRemux, caps), info.subtitles[0]), null);

  const burned = S.plan(ctx(sdrMovie, caps, { subtitle: 2 }));
  assert.equal(burned.method, 'Transcode');
  assert.equal(params(burned.url).get('SubtitleStreamIndex'), '2');
  assert.equal(params(burned.url).get('SubtitleMethod'), 'Encode');
  assert.deepEqual([...burned.reasons], ['burning in subtitles']);
});

test('HDR films offer no lower qualities and label burned-in subtitles as SDR', () => {
  const hdr = S.sourceInfo(hdrRemux);
  assert.deepEqual([...S.qualities(hdr)], []);
  assert.equal(S.subtitleLabel(hdr, hdr.subtitles[0]), 'English PGS · burned in, SDR');
  assert.equal(S.subtitleLabel(hdr, hdr.subtitles[1]), 'English SRT');
  const sdr = S.sourceInfo(sdrMovie);
  // 12 Mbps 1080p source: only qualities below it.
  assert.deepEqual(
    [...S.qualities(sdr).map((q) => q.name)],
    ['1080p · 10 Mbps', '720p · 6 Mbps', '720p · 3 Mbps', '480p · 1.5 Mbps'],
  );
  assert.equal(S.subtitleLabel(sdr, sdr.subtitles[0]), 'French · burned in');
});

test('a chosen lower quality transcodes at that bitrate and height', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(sdrMovie, caps, { quality: { bitrate: 6000000, height: 720 } }));
  assert.equal(p.method, 'Transcode');
  const q = params(p.url);
  assert.equal(q.get('VideoCodec'), 'h264');
  assert.equal(q.get('VideoBitrate'), '6000000');
  assert.equal(q.get('MaxHeight'), '720');
  // The audio the TV decodes is still copied.
  assert.equal(q.get('AudioCodec').split(',')[0], 'eac3');
});

test('a video codec the TV cannot decode is the only video transcode', () => {
  const vc1 = {
    ...sdrMovie,
    MediaStreams: [stream(0, 'Video', 'vc1', { VideoRangeType: 'SDR' }), sdrMovie.MediaStreams[1]],
  };
  const p = S.plan(ctx(vc1, S.capabilities(tv())));
  assert.equal(p.method, 'Transcode');
  assert.deepEqual([...p.reasons], ['VC-1 video not supported']);
});

test('a failed direct play can be retried as a stream copy', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(sdrMovie, caps, { forceStream: true }));
  assert.equal(p.method, 'DirectStream');
  assert.equal(params(p.url).get('AllowVideoStreamCopy'), 'true');
});

// From a debug log (webOS, 2026-11): HEVC with FLAC over HLS fails at once
// with "format error". DTS and TrueHD films now stream as one MKV.
test('DTS or TrueHD audio streams as one MKV: video copied, audio to FLAC', () => {
  const caps = S.capabilities(tv());
  const dts = {
    ...hdrRemux,
    MediaStreams: [
      hdrRemux.MediaStreams[0],
      stream(1, 'Audio', 'dts', { IsDefault: true, Channels: 6 }),
    ],
  };
  for (const source of [hdrRemux, dts]) {
    const p = S.plan(ctx(source, caps));
    assert.equal(p.method, 'DirectStream');
    assert.equal(p.progressive, true);
    const url = new URL(p.url);
    assert.match(url.pathname, /\/Videos\/item1\/stream\.mkv$/);
    const q = url.searchParams;
    assert.equal(q.get('static'), 'false');
    assert.equal(q.get('VideoCodec'), 'hevc');
    assert.equal(q.get('AllowVideoStreamCopy'), 'true');
    assert.equal(q.get('AudioCodec').split(',')[0], 'flac');
    assert.equal(q.get('AudioStreamIndex'), '1');
    assert.equal(q.get('SegmentContainer'), null, 'not HLS');
    assert.equal(q.get('StartTimeTicks'), null, 'from the start');
    assert.equal(p.audioPosition, 0, 'one audio track in the stream');
    assert.match(p.label, /^Direct Stream · HDR10 · (TrueHD|DTS) → FLAC$/);
  }
});

test('a continuous stream starts where the picture is', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(hdrRemux, caps, { start: 674.5 }));
  assert.equal(params(p.url).get('StartTimeTicks'), String(674.5 * 1e7));
});

test('when the TV refuses the MKV stream, HLS copies the video and never sends FLAC', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(hdrRemux, caps, { hls: true, start: 60 }));
  assert.equal(p.method, 'DirectStream');
  assert.equal(p.progressive, false);
  assert.match(new URL(p.url).pathname, /master\.m3u8$/);
  const q = params(p.url);
  assert.equal(q.get('AllowVideoStreamCopy'), 'true', 'HDR stays HDR');
  assert.equal(q.get('SegmentContainer'), 'mp4');
  assert.ok(!q.get('AudioCodec').includes('flac'), q.get('AudioCodec'));
  assert.equal(q.get('AudioCodec'), 'eac3,ac3,aac');
  assert.equal(q.get('TranscodingMaxAudioChannels'), '6');
  assert.equal(p.label, 'Direct Stream (HLS) · HDR10 · TrueHD → E-AC-3');
});

test('a video re-encode is still HLS, and its audio is never FLAC either', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(sdrMovie, caps, { subtitle: 2 }));
  assert.equal(p.method, 'Transcode');
  assert.match(new URL(p.url).pathname, /master\.m3u8$/);
  assert.ok(!params(p.url).get('AudioCodec').includes('flac'));
});
