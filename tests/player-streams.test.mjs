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
  assert.equal(params(p.url).get('ApiKey'), 'tok');
  assert.equal(p.label, 'Direct Play · E-AC-3');
});

test('HDR with TrueHD 7.1 picked: the video is copied in HDR, the audio becomes E-AC-3 5.1', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(hdrRemux, caps, { audio: 1 }));
  assert.equal(p.method, 'DirectStream');
  assert.match(new URL(p.url).pathname, /main\.m3u8$/);
  const q = params(p.url);
  assert.equal(q.get('VideoCodec'), 'hevc');
  assert.equal(q.get('AllowVideoStreamCopy'), 'true');
  assert.equal(q.get('SegmentContainer'), 'mp4');
  // No limits that would stop Jellyfin from copying the video.
  for (const key of ['VideoBitrate', 'MaxHeight', 'MaxWidth', 'SubtitleStreamIndex']) {
    assert.equal(q.get(key), null, key);
  }
  // HDR10 is accepted as it is (copied); dual-layer Dolby Vision is not, so
  // Jellyfin strips it to the HDR10 base.
  const ranges = q.get('hevc-rangetype').split(',');
  assert.ok(ranges.includes('HDR10') && ranges.includes('DOVIWithHDR10'));
  assert.ok(ranges.includes('DOVI') && !ranges.includes('DOVIWithEL'));
  // webOS refuses surround FLAC (Jellyfin's own webOS profile says so too).
  assert.equal(q.get('AudioCodec'), 'eac3,ac3,aac');
  assert.equal(q.get('AudioStreamIndex'), '1');
  assert.equal(q.get('TranscodingMaxAudioChannels'), '6');
  assert.equal(p.audioOut, 'eac3');
  assert.equal(p.fallbackLeft, true);
  assert.equal(p.label, 'Direct Stream · HDR10 · TrueHD → E-AC-3');
});

test('a stereo track the TV cannot decode becomes FLAC, lossless', () => {
  const caps = S.capabilities(tv());
  const stereo = {
    ...hdrRemux,
    MediaStreams: [
      hdrRemux.MediaStreams[0],
      stream(1, 'Audio', 'dts', { IsDefault: true, Channels: 2 }),
    ],
  };
  const q = params(S.plan(ctx(stereo, caps)).url);
  assert.equal(q.get('AudioCodec'), 'flac,eac3,ac3,aac');
  assert.equal(q.get('TranscodingMaxAudioChannels'), '2');
});

test('surround FLAC in the file is converted, not played as it is', () => {
  const caps = S.capabilities(tv());
  const flac = {
    ...sdrMovie,
    MediaStreams: [
      sdrMovie.MediaStreams[0],
      stream(1, 'Audio', 'flac', { IsDefault: true, Channels: 6 }),
    ],
  };
  const p = S.plan(ctx(flac, caps));
  assert.equal(p.method, 'DirectStream');
  assert.equal(params(p.url).get('AudioCodec'), 'eac3,ac3,aac');
  const two = {
    ...flac,
    MediaStreams: [
      sdrMovie.MediaStreams[0],
      stream(1, 'Audio', 'flac', { IsDefault: true, Channels: 2 }),
    ],
  };
  assert.equal(S.plan(ctx(two, caps)).method, 'DirectPlay');
});

test('when the TV refuses a converted stream, the audio steps down to AC-3, then AAC', () => {
  const caps = S.capabilities(tv());
  const first = S.plan(ctx(hdrRemux, caps, { audio: 1, fallback: 1 }));
  assert.equal(params(first.url).get('AudioCodec'), 'ac3,aac');
  assert.equal(params(first.url).get('AllowVideoStreamCopy'), 'true', 'still HDR');
  assert.equal(first.fallbackLeft, true);
  const last = S.plan(ctx(hdrRemux, caps, { audio: 1, fallback: 2 }));
  assert.equal(params(last.url).get('AudioCodec'), 'aac');
  assert.equal(last.fallbackLeft, false);
});

test('a TV set to play TrueHD gets the file as it is', () => {
  const caps = S.capabilities(tv(), { truehd: 'on' });
  assert.equal(caps.audio.truehd, true);
  const p = S.plan(ctx(hdrRemux, caps));
  assert.equal(p.method, 'DirectPlay');
  assert.equal(p.label, 'Direct Play · HDR10 · TrueHD');
  // And one set to convert it never does, whatever it answers.
  const off = S.capabilities(tv(['audio/mp4; codecs="mlpa"']), { truehd: 'off' });
  assert.equal(off.audio.truehd, false);
  const asked = S.capabilities(tv(['audio/mp4; codecs="mlpa"']), { truehd: 'auto' });
  assert.equal(asked.audio.truehd, true, 'Ask the TV: its answer counts');
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
  const p = S.plan(ctx(hdrRemux, caps, { audio: 1 }));
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
  // AVI is no direct-play container: remuxed over HLS, video and DTS copied.
  assert.equal(p.method, 'DirectStream');
  assert.match(new URL(p.url).pathname, /main\.m3u8$/);
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

// A TV like the 2021 OLED, but without Dolby Digital Plus.
const noEac3 = () => tv([], ['audio/mp4; codecs="ec-3"']);

test('TrueHD the TV cannot decode is re-encoded to E-AC-3, not swapped for the AC-3 track', () => {
  const caps = S.capabilities(tv());
  const p = S.plan(ctx(hdrRemux, caps));
  assert.equal(p.method, 'DirectStream');
  assert.equal(p.audioIndex, 1);
  assert.equal(p.audioSwappedFrom, null);
  const q = params(p.url);
  assert.equal(q.get('AudioStreamIndex'), '1');
  assert.equal(q.get('AudioCodec').split(',')[0], 'eac3');
  assert.equal(q.get('AllowVideoStreamCopy'), 'true', 'the HDR video is still copied');
  assert.equal(p.label, 'Direct Stream · HDR10 · TrueHD → E-AC-3');
});

test('without E-AC-3 on the TV, the TrueHD default gives way to its AC-3 companion', () => {
  const caps = S.capabilities(noEac3());
  const p = S.plan(ctx(hdrRemux, caps));
  assert.equal(p.method, 'DirectPlay');
  assert.equal(p.audioIndex, 2);
  assert.equal(p.audioPosition, 1);
  assert.equal(p.audioSwappedFrom, 'TrueHD');
  assert.equal(p.label, 'Direct Play · HDR10 · AC-3 (in place of TrueHD)');
});

const withTracks = (video, ...audio) => ({
  Id: 'src3',
  Container: 'mkv',
  MediaStreams: [video].concat(audio.map((a, i) => stream(i + 1, 'Audio', a[0], a[1]))),
});
const hevc = stream(0, 'Video', 'hevc', { VideoRange: 'HDR', VideoRangeType: 'HDR10' });

const dtsFilm = withTracks(
  hevc,
  ['dts', { IsDefault: true, Channels: 8, Language: 'eng', DisplayTitle: 'DTS-HD MA 7.1' }],
  ['eac3', { Channels: 6, Language: 'ger' }],
  ['ac3', { Channels: 2, Language: 'eng', Title: 'Director Commentary' }],
  ['ac3', { Channels: 6, Language: 'eng' }],
  ['aac', { Channels: 2, Language: 'eng' }],
);

test('the DTS-HD MA track wins and is converted to E-AC-3 when the TV has it', () => {
  const p = S.plan(ctx(dtsFilm, S.capabilities(tv())));
  assert.equal(p.audioIndex, 1);
  assert.equal(p.method, 'DirectStream');
  assert.match(p.label, /DTS → E-AC-3/);
});

test('the fallback keeps the language, skips commentary and prefers more channels', () => {
  const p = S.plan(ctx(dtsFilm, S.capabilities(noEac3())));
  assert.equal(p.audioIndex, 4, 'English AC-3 5.1');
  assert.equal(p.method, 'DirectPlay');
  assert.match(p.label, /AC-3 \(in place of DTS\)/);
});

test('with nothing picked, the first English track in the highest quality plays', () => {
  const caps = S.capabilities(tv(['audio/mp4; codecs="mlpa"']));
  const film = withTracks(
    hevc,
    ['ac3', { IsDefault: true, Channels: 6, Language: 'fre' }],
    ['ac3', { Channels: 6, Language: 'eng', BitRate: 448000 }],
    ['truehd', { Channels: 8, Language: 'eng', Profile: 'TrueHD + Atmos' }],
    ['ac3', { Channels: 6, Language: 'eng', BitRate: 640000 }],
    ['flac', { Channels: 2, Language: 'eng', Title: 'Commentary' }],
  );
  const p = S.plan(ctx(film, caps));
  assert.equal(p.audioIndex, 3, 'the TrueHD track, not the French default');
  assert.equal(p.method, 'DirectPlay');
  // A track the viewer picks is still played as picked.
  assert.equal(S.plan(ctx(film, caps, { audio: 1 })).audioIndex, 1);
});

test('track quality: lossless, then channels, then codec, then bitrate, then order', () => {
  const info = S.sourceInfo(
    withTracks(
      hevc,
      ['eac3', { Channels: 2, Language: 'eng' }],
      ['ac3', { Channels: 6, Language: 'eng', BitRate: 448000 }],
      ['ac3', { Channels: 6, Language: 'eng', BitRate: 640000 }],
      ['ac3', { Channels: 6, Language: 'eng', BitRate: 640000 }],
      ['dts', { Channels: 2, Language: 'eng', Profile: 'DTS-HD MA' }],
    ),
  );
  assert.equal(S.bestTrack(info.audio).index, 5);
  assert.equal(S.bestTrack(info.audio.slice(0, 4)).index, 3);
  const german = S.sourceInfo(
    withTracks(hevc, ['ac3', { Language: 'ger' }], ['aac', { Language: 'fre', IsDefault: true }]),
  );
  assert.equal(S.bestTrack(german.audio).index, 2, 'no English: the default');
});

test('Plex tracks: English DTS-HD MA chosen, Dolby targets only the TV decodes', () => {
  const audio = S.plexAudio([
    { streamType: 1, id: 1, codec: 'hevc' },
    { streamType: 2, id: 11, codec: 'ac3', languageCode: 'eng', channels: 6, bitrate: 640 },
    { streamType: 2, id: 12, codec: 'dca', profile: 'ma', languageCode: 'eng', channels: 8 },
  ]);
  assert.equal(audio[0].bitrate, 640000);
  assert.equal(S.bestTrack(audio).index, 12);
  const extra = S.plexProfileExtra(S.capabilities(tv()));
  assert.match(
    extra,
    /^add-transcode-target-audio-codec\(.*audioCodec=eac3\)\+.*audioCodec=ac3\)$/,
  );
  assert.equal(
    S.plexProfileExtra(
      S.capabilities(tv([], ['audio/mp4; codecs="ec-3"', 'audio/mp4; codecs="ac-3"'])),
    ),
    '',
  );
});

test('with no playable track in the language, the default is converted as before', () => {
  const caps = S.capabilities(tv());
  const film = withTracks(
    hevc,
    ['truehd', { IsDefault: true, Channels: 8, Language: 'eng' }],
    ['ac3', { Channels: 6, Language: 'fre' }],
    ['ac3', { Channels: 2, Language: 'eng', Title: 'Commentary' }],
  );
  const p = S.plan(ctx(film, caps));
  assert.equal(p.audioSwappedFrom, null);
  assert.equal(p.method, 'DirectStream');
  assert.equal(params(p.url).get('AudioStreamIndex'), '1');
  assert.match(p.label, /TrueHD → E-AC-3/);
});

test('dual-layer Dolby Vision streams (to strip the layer) with the AC-3 companion copied', () => {
  const caps = S.capabilities(noEac3());
  const film = withTracks(
    stream(0, 'Video', 'hevc', { VideoRange: 'HDR', VideoRangeType: 'DOVIWithEL' }),
    ['truehd', { IsDefault: true, Channels: 8, Language: 'eng' }],
    ['ac3', { Channels: 6, Language: 'eng' }],
  );
  const p = S.plan(ctx(film, caps));
  assert.equal(p.method, 'DirectStream');
  const q = params(p.url);
  assert.equal(q.get('AudioStreamIndex'), '2');
  assert.equal(q.get('AudioCodec').split(',')[0], 'ac3', 'the AC-3 track is copied');
  assert.equal(p.audioSwappedFrom, 'TrueHD');
});

const jfCtx = { base: 'http://jf:8096', item: 'item1', token: 'tok' };

test('chapters come sorted with their images, and a single chapter is no list', () => {
  const list = S.chapters(
    {
      Chapters: [
        { StartPositionTicks: 6000000000, Name: 'The Iceberg', ImageTag: 'b' },
        { StartPositionTicks: 0, Name: '', ImageTag: 'a' },
        { StartPositionTicks: 12000000000, Name: 'Sinking' },
      ],
    },
    jfCtx,
  );
  assert.deepEqual(JSON.parse(JSON.stringify(list.map((c) => [c.start, c.name]))), [
    [0, 'Chapter 2'],
    [600, 'The Iceberg'],
    [1200, 'Sinking'],
  ]);
  assert.match(list[1].image, /\/Items\/item1\/Images\/Chapter\/0\?tag=b/);
  assert.equal(list[2].image, null);
  assert.equal(S.chapterAt(list, 599), 0);
  assert.equal(S.chapterAt(list, 600), 1);
  assert.equal(S.chapterAt(list, 5000), 2);
  assert.equal(S.chapters({ Chapters: [{ StartPositionTicks: 0 }] }, jfCtx).length, 0);
  assert.equal(S.chapters(null, jfCtx).length, 0);
});

test('trickplay picks the set nearest 320 px and finds the frame in its tile', () => {
  const info = (Width) => ({
    Width,
    Height: Math.round((Width * 9) / 16),
    TileWidth: 10,
    TileHeight: 10,
    ThumbnailCount: 500,
    Interval: 10000,
  });
  const tp = S.trickplay(
    { Trickplay: { src1: { 160: info(160), 320: info(320), 640: info(640) } } },
    jfCtx,
    'src1',
  );
  assert.equal(tp.width, 320);
  // 1234 s = frame 123: tile 1, frame 23 in it (row 2, column 3).
  const f = S.trickplayFrame(tp, 1234, 320);
  assert.equal(
    f.url,
    'http://jf:8096/Videos/item1/Trickplay/320/1.jpg?MediaSourceId=src1&ApiKey=tok',
  );
  assert.equal(f.position, '-960px -360px');
  assert.equal(f.size, '3200px 1800px');
  assert.equal(f.height, 180);
  // Past the end: the last frame, never a tile that does not exist.
  assert.match(S.trickplayFrame(tp, 999999, 320).url, /\/4\.jpg/);
  assert.equal(S.trickplay({}, jfCtx, 'src1'), null);
});

test('trickplay imported by Jellyfin counts tiles, not frames: the film length decides', () => {
  const tp = S.trickplay(
    {
      RunTimeTicks: 8178 * 1e7,
      Trickplay: {
        src1: {
          320: { Width: 320, TileWidth: 10, TileHeight: 10, ThumbnailCount: 41, Interval: 2000 },
        },
      },
    },
    jfCtx,
    'src1',
  );
  assert.equal(tp.reported, 41);
  assert.equal(tp.count, 4089);
  assert.equal(tp.height, 180, '16:9 when Jellyfin stores no height');
  // 1:43:53 is frame 3116: tile 31, not stuck on the first tiles.
  assert.equal(S.trickplayTile(tp, 6233), 31);
  assert.match(S.trickplayFrame(tp, 6233, 320).url, /\/31\.jpg/);
  // A correct count is left alone.
  const ok = S.trickplay(
    {
      RunTimeTicks: 8178 * 1e7,
      Trickplay: {
        src1: {
          320: {
            Width: 320,
            Height: 136,
            TileWidth: 10,
            TileHeight: 10,
            ThumbnailCount: 4089,
            Interval: 2000,
          },
        },
      },
    },
    jfCtx,
    'src1',
  );
  assert.equal(ok.count, 4089);
  assert.equal(ok.reported, 4089);
});

test('HDR controls: 150 nits on a TV drawing graphics white at 400 nits', () => {
  // brightness() scales gamma-encoded values: (150/400)^(1/2.2).
  assert.ok(Math.abs(S.hdrBrightness(150, 400) - 0.6405) < 0.001);
  assert.ok(Math.abs(S.hdrBrightness(150, 203) - 0.8715) < 0.001);
  // Back to linear light the controls land on the level asked for.
  assert.ok(Math.abs(Math.pow(S.hdrBrightness(120, 600), 2.2) * 600 - 120) < 0.01);
  assert.equal(S.hdrBrightness(300, 250), 1, 'never brighter than the TV');
  assert.equal(S.hdrBrightness(0, 400), 1);
});

test('trickplay asks for the version that has it, by its own ID', () => {
  const item = {
    RunTimeTicks: 7508e7,
    Trickplay: {
      other: {
        320: {
          Width: 320,
          Height: 136,
          TileWidth: 10,
          TileHeight: 10,
          Interval: 2000,
          ThumbnailCount: 3755,
        },
      },
    },
  };
  const tp = S.trickplay(item, { base: 'http://jf', item: 'item1', token: 'tok' }, 'playing');
  assert.equal(tp.owner, 'other');
  assert.match(
    tp.tile(0),
    /\/Videos\/item1\/Trickplay\/320\/0\.jpg\?MediaSourceId=other&ApiKey=tok$/,
  );
  assert.equal(tp.tileDirect(3), 'http://jf/Videos/other/Trickplay/320/3.jpg?ApiKey=tok');
  assert.ok(!tp.where(false).includes('tok') && !tp.where(true).includes('tok'));
  tp.direct = true;
  assert.match(S.trickplayFrame(tp, 0, 320).url, /\/Videos\/other\/Trickplay\//);
});

test('dual-layer Dolby Vision is labelled by what plays', () => {
  assert.equal(S.rangeName('DOVIWithEL', true), 'HDR10 (Dolby Vision layer removed)');
  assert.equal(S.rangeName('DOVIWithEL', false), 'Dolby Vision');
  assert.equal(S.rangeName('DOVIWithHDR10', true), 'Dolby Vision');
  assert.equal(S.rangeName('HDR10Plus', true), 'HDR10+');
  assert.equal(S.rangeName('HDR10', false), 'HDR10');
});
