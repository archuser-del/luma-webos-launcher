import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

// The player's audio and HDR choices: media.js runs as a plain script on the TV.
const source = fs.readFileSync(
  path.join(import.meta.dirname, '../extras/player/app/media.js'),
  'utf8',
);
const sandbox = { window: {} };
vm.runInNewContext(source, sandbox);
const M = sandbox.window.LumaMedia;

const jf = (index, codec, extra = {}) => ({
  Type: 'Audio',
  Index: index,
  Codec: codec,
  Language: 'eng',
  Channels: 6,
  ...extra,
});
const every = () => true;
const only =
  (...codecs) =>
  (c) =>
    codecs.includes(c);

test('the best English track wins, whatever its place', () => {
  const tracks = M.fromJellyfin([
    { Type: 'Video', Index: 0, Codec: 'hevc' },
    jf(1, 'ac3', { Language: 'fre', Channels: 6 }),
    jf(2, 'ac3', { BitRate: 448000 }),
    jf(3, 'truehd', { Channels: 8, Profile: 'Dolby TrueHD + Dolby Atmos' }),
    jf(4, 'ac3', { Channels: 2, Title: 'Commentary with the director' }),
  ]);
  const plan = M.plan(tracks, { direct: false, tvPlays: every });
  assert.equal(plan.track.index, 3);
});

test('TrueHD is re-encoded to DD+ rather than dropping to the DD track', () => {
  const tracks = M.fromJellyfin([jf(1, 'truehd', { Channels: 8 }), jf(2, 'ac3')]);
  const plan = M.plan(tracks, { direct: false, tvPlays: only('eac3', 'ac3', 'aac') });
  assert.equal(plan.track.index, 1);
  assert.equal(plan.hls, true);
  assert.equal(plan.convert, true);
  assert.equal(plan.codecs[0], 'eac3');
  const url = M.jellyfinUrl(
    'https://jf.invalid/Videos/x/master.m3u8?ApiKey=k&AudioCodec=aac,ac3,eac3,mp3&TranscodingMaxAudioChannels=6',
    plan.track,
    plan.codecs,
  );
  assert.equal(
    url,
    'https://jf.invalid/Videos/x/master.m3u8?ApiKey=k&AudioCodec=eac3%2Cac3%2Caac&TranscodingMaxAudioChannels=6&AudioStreamIndex=1',
  );
});

test('without DD+ on the TV, the track it plays as is beats a re-encode', () => {
  const tracks = M.fromJellyfin([jf(1, 'truehd', { Channels: 8 }), jf(2, 'ac3')]);
  const plan = M.plan(tracks, { direct: false, tvPlays: only('ac3', 'aac') });
  assert.equal(plan.track.index, 2);
  assert.equal(plan.convert, false);
  assert.deepEqual([...plan.codecs], ['ac3', 'aac']);
});

test('codecs are only requested after the TV says it plays them', () => {
  const asked = [];
  const codecs = M.audioCodecs((c) => {
    asked.push(c);
    return c === 'ac3';
  });
  assert.ok(asked.includes('eac3'));
  // No DD+ on this TV: AC-3 is the encode target, AAC the last resort.
  assert.deepEqual([...codecs], ['ac3', 'aac']);
});

test('a source the TV plays is copied, not converted', () => {
  const tracks = M.fromJellyfin([jf(1, 'eac3', { Profile: 'Dolby Digital Plus + Dolby Atmos' })]);
  const plan = M.plan(tracks, { direct: false, tvPlays: only('eac3', 'aac') });
  assert.equal(plan.convert, false);
  assert.equal(plan.hls, true);
});

test('a file plays as is when the TV plays the chosen track and can switch to it', () => {
  const tracks = M.fromJellyfin([jf(1, 'aac', { Channels: 2 }), jf(2, 'eac3')]);
  const direct = M.plan(tracks, { direct: true, switchable: true, tvPlays: every });
  assert.equal(direct.hls, false);
  assert.equal(direct.track.pos, 1);
  // Without track switching in the TV's browser the file can't reach track 2.
  const hls = M.plan(tracks, { direct: true, switchable: false, tvPlays: every });
  assert.equal(hls.hls, true);
  assert.equal(hls.track.index, 2);
});

test('lossless beats lossy, then channels, then codec, then bitrate, then order', () => {
  const tracks = M.fromJellyfin([
    jf(1, 'eac3', { Channels: 2 }),
    jf(2, 'ac3', { Channels: 6, BitRate: 448000 }),
    jf(3, 'ac3', { Channels: 6, BitRate: 640000 }),
    jf(4, 'dts', { Channels: 2, Profile: 'DTS-HD MA' }),
    jf(5, 'ac3', { Channels: 6, BitRate: 640000 }),
  ]);
  assert.deepEqual(
    M.rank(tracks).map((t) => t.index),
    [4, 3, 5, 2, 1],
  );
});

test('no English track: the server default plays', () => {
  const tracks = M.fromJellyfin([
    jf(1, 'ac3', { Language: 'ger' }),
    jf(2, 'ac3', { Language: 'fre', IsDefault: true }),
  ]);
  assert.equal(M.plan(tracks, { direct: false, tvPlays: every }).track.index, 2);
});

test('commentary plays only when nothing else is English', () => {
  const tracks = M.fromJellyfin([
    jf(1, 'flac', { Channels: 2, DisplayTitle: 'English - Commentary - FLAC - Stereo' }),
    jf(2, 'aac', { Channels: 2 }),
  ]);
  assert.equal(M.plan(tracks, { direct: false, tvPlays: every }).track.index, 2);
});

test('Plex streams: DTS-HD MA, English, kbit/s, and HDR', () => {
  const info = M.describe({
    service: 'plex',
    part: 4417,
    hls: 'https://plex.invalid/start.m3u8?path=%2Flibrary%2Fmetadata%2F9&session=s',
    streams: [
      { streamType: 1, id: 10, codec: 'hevc', colorTrc: 'smpte2084' },
      { streamType: 2, id: 11, codec: 'ac3', languageCode: 'eng', channels: 6, bitrate: 640 },
      { streamType: 2, id: 12, codec: 'dca', profile: 'ma', languageCode: 'eng', channels: 8 },
    ],
  });
  assert.equal(info.hdr, true);
  assert.equal(info.part, '4417');
  assert.equal(info.tracks[0].bitrate, 640000);
  const plan = M.plan(info.tracks, { direct: false, tvPlays: only('eac3', 'ac3', 'aac') });
  assert.equal(plan.track.index, 12);
  assert.equal(plan.convert, true);
  const url = M.plexUrl(info.hls, plan.codecs);
  assert.match(
    url,
    /^https:\/\/plex\.invalid\/start\.m3u8\?path=%2Flibrary%2Fmetadata%2F9&session=s&/,
  );
  assert.match(url, /audioCodec%3Deac3\)%2Badd-transcode-target-audio-codec/);
});

test('HDR comes from the video stream only', () => {
  const hdr = (streams) => M.describe({ service: 'jellyfin', streams }).hdr;
  assert.equal(hdr([{ Type: 'Video', VideoRange: 'HDR', VideoRangeType: 'DOVI' }]), true);
  assert.equal(hdr([{ Type: 'Video', VideoRange: 'SDR', VideoRangeType: 'SDR' }]), false);
  assert.equal(hdr([{ Type: 'Audio', VideoRange: 'HDR' }]), false);
});
