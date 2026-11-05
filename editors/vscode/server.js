// @ts-check
// Downloads, verifies and installs `stirphp` from GitHub Releases. Independent of
// the VS Code API, so it can be tested with plain Node.

import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { chmod, mkdir, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join, sep } from 'node:path';
import { promisify } from 'node:util';
import { unzipSync } from 'fflate';
import * as tar from 'tar';

const REPO = 'StirPHP/core';
const run = promisify(execFile);

/** @typedef {'stable' | 'nightly'} Channel */

/**
 * A release of `stirphp` for this platform.
 * @typedef {object} Release
 * @property {string} id         Identifies the build: the tag for stable, the commit for nightly
 * @property {string} label      Human-readable version
 * @property {string} archive    Download URL of the archive
 * @property {string} archiveName
 * @property {string} sums       Download URL of SHA256SUMS
 */

/**
 * The installed build.
 * @typedef {object} Installed
 * @property {Channel} channel
 * @property {string} id
 * @property {string} label
 * @property {string} path       The `stirphp` executable
 */

/** The Rust target `stirphp` is built for on this machine, or null if there's no build. */
export function targetTriple(platform = process.platform, arch = process.arch) {
  /** @type {Record<string, string>} */
  const targets = {
    'linux-x64': 'x86_64-unknown-linux-musl',
    'linux-arm64': 'aarch64-unknown-linux-musl',
    'darwin-x64': 'x86_64-apple-darwin',
    'darwin-arm64': 'aarch64-apple-darwin',
    'win32-x64': 'x86_64-pc-windows-msvc',
    'win32-arm64': 'aarch64-pc-windows-msvc',
  };
  return targets[`${platform}-${arch}`] ?? null;
}

/** @param {string} url */
async function fetchOk(url, accept = 'application/octet-stream') {
  const res = await fetch(url, {
    headers: { accept, 'user-agent': 'stirphp-vscode', 'x-github-api-version': '2022-11-28' },
    redirect: 'follow',
  });
  if (!res.ok) throw new Error(`GET ${url}: ${res.status} ${res.statusText}`);
  return res;
}

/**
 * The newest release on `channel` with a build for `target`.
 * @param {Channel} channel
 * @param {string} target
 * @returns {Promise<Release>}
 */
export async function latestRelease(channel, target) {
  const api = `https://api.github.com/repos/${REPO}/releases/${channel === 'nightly' ? 'tags/nightly' : 'latest'}`;
  /** @type {{ tag_name: string, target_commitish: string, assets?: { name: string, browser_download_url: string }[] }} */
  const release = /** @type {any} */ (await (await fetchOk(api, 'application/vnd.github+json')).json());
  const assets = release.assets ?? [];
  const archive = assets.find((a) => a.name.startsWith('stirphp-') && a.name.includes(`-${target}.`));
  const sums = assets.find((a) => a.name === 'SHA256SUMS');
  if (!archive || !sums) throw new Error(`${release.tag_name} has no stirphp build for ${target}`);
  // nightlies reuse their tag: the commit identifies the build
  const id = channel === 'nightly' ? String(release.target_commitish) : String(release.tag_name);
  const label = channel === 'nightly' ? `nightly (${id.slice(0, 7)})` : String(release.tag_name).replace(/^v/, '');
  return { id, label, archive: archive.browser_download_url, archiveName: archive.name, sums: sums.browser_download_url };
}

/**
 * Downloads `release` into `dir`/<build>/, verifies its checksum, unpacks it
 * and runs it once.
 * @param {Release} release
 * @param {Channel} channel
 * @param {string} dir   The extension's storage for binaries
 * @param {{ verify?: boolean }} [options]  `verify: false` skips running it (tests)
 * @returns {Promise<Installed>}
 */
export async function install(release, channel, dir, { verify = true } = {}) {
  const [archive, sums] = await Promise.all([
    fetchOk(release.archive).then((r) => r.arrayBuffer()).then((b) => Buffer.from(b)),
    fetchOk(release.sums).then((r) => r.text()),
  ]);

  const want = sums
    .split('\n')
    .map((l) => l.trim().split(/\s+/))
    .find(([, name]) => name?.replace(/^\*/, '') === release.archiveName)?.[0];
  const got = createHash('sha256').update(archive).digest('hex');
  if (!want) throw new Error(`SHA256SUMS doesn't list ${release.archiveName}`);
  if (want !== got) throw new Error(`checksum mismatch for ${release.archiveName}`);

  const safe = release.id.replace(/[^\w.-]/g, '_');
  const target = join(dir, `${channel}-${safe}`);
  const staging = `${target}.partial`;
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true });

  // Windows builds come as .zip with stirphp.exe
  const zip = release.archiveName.endsWith('.zip');
  const exe = zip ? 'stirphp.exe' : 'stirphp';
  if (zip) {
    const files = unzipSync(new Uint8Array(archive), { filter: (f) => f.name.endsWith(`/${exe}`) || f.name === exe });
    const entry = Object.entries(files)[0];
    if (!entry) throw new Error(`${release.archiveName} has no ${exe}`);
    await writeFile(join(staging, exe), entry[1]);
  } else {
    const archivePath = join(staging, release.archiveName);
    await writeFile(archivePath, archive);
    // the archive holds one directory: stirphp-<name>-<target>/stirphp
    await tar.x({ file: archivePath, cwd: staging, strip: 1, filter: (p) => p.endsWith(`/${exe}`) });
    await rm(archivePath);
  }
  const path = join(staging, exe);
  await chmod(path, 0o755);
  if (verify) await run(path, ['--version']);

  await rm(target, { recursive: true, force: true });
  await rename(staging, target);
  const installed = { channel, id: release.id, label: release.label, path: join(target, exe) };
  await writeFile(join(dir, 'installed.json'), JSON.stringify(installed, null, 2));
  return installed;
}

/**
 * The build recorded as installed in `dir`, if its executable is still there.
 * @param {string} dir
 * @returns {Promise<Installed | null>}
 */
export async function installed(dir) {
  try {
    /** @type {Installed} */
    const info = JSON.parse(await readFile(join(dir, 'installed.json'), 'utf8'));
    await run(info.path, ['--version']);
    return info;
  } catch {
    return null;
  }
}

/**
 * Removes builds other than `keep` (old versions, interrupted downloads).
 * Builds in use can't be removed on Windows; they go on a later run.
 * @param {string} dir
 * @param {Installed} keep
 */
export async function cleanup(dir, keep) {
  let entries = [];
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const e of entries) {
    if (e.isDirectory() && !keep.path.startsWith(join(dir, e.name) + sep)) {
      await rm(join(dir, e.name), { recursive: true, force: true }).catch(() => {});
    }
  }
}
