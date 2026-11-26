// Produce the independent companion web apps with the official webOS CLI:
// Luma Player (Jellyfin and Plex playback) and Luma Aerials (the full-screen
// video library, Settings › System).
import fs from 'node:fs/promises';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const root = path.resolve(import.meta.dirname, '..');
const companions = [
  ['extras/player/app', 'com.archbtw.luma.player', 'Luma Player'],
  ['extras/aerials', 'com.archbtw.luma.aerials', 'Luma Aerials'],
];
const output = path.join(root, 'dist');
await fs.mkdir(output, { recursive: true });
const state = path.join(root, '.build/cli-state');
await fs.mkdir(state, { recursive: true });
for (const [folder, id, name] of companions) {
  const directory = path.join(root, folder);
  const manifest = JSON.parse(await fs.readFile(path.join(directory, 'appinfo.json'), 'utf8'));
  if (manifest.id !== id || manifest.type !== 'web') {
    throw new Error(`Unexpected ${name} identity or app type`);
  }
  const result = spawnSync(
    process.execPath,
    [
      path.join(root, 'node_modules/@webos-tools/cli/bin/ares-package.js'),
      '--no-minify',
      '--outdir',
      output,
      directory,
    ],
    { cwd: root, env: { ...process.env, APPDATA: state }, stdio: 'inherit', timeout: 120000 },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
  const filename = `${manifest.id}_${manifest.version}_all.ipk`;
  const bytes = await fs.readFile(path.join(output, filename));
  if (bytes.subarray(0, 8).toString() !== '!<arch>\n') throw new Error(`Invalid ${name} IPK`);
  const hash = createHash('sha256').update(bytes).digest('hex');
  const checksumsPath = path.join(output, 'sha256sums');
  const previous = await fs.readFile(checksumsPath, 'utf8');
  const otherLines = previous
    .split('\n')
    .filter((line) => line.trim() && !line.includes(`  ${manifest.id}_`));
  await fs.writeFile(checksumsPath, [...otherLines, `${hash}  ${filename}`].join('\n') + '\n');
  console.log(`Companion: dist/${filename} (${bytes.length} bytes) SHA-256: ${hash}`);
}
