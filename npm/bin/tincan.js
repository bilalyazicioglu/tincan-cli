#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const os = require('os');
const https = require('https');
const crypto = require('crypto');
const { spawnSync, execSync } = require('child_process');

const pkg = require('../package.json');
const VERSION = pkg.version;

const PLATFORMS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

const isWindows = process.platform === 'win32';

const key = `${process.platform}-${process.arch}`;
const target = PLATFORMS[key];

if (!target) {
  console.error(`\x1b[31m[tincan]\x1b[0m Unsupported platform: ${key}.`);
  console.error(`Prebuilt binaries are available for macOS (arm64, x64), Linux (x64, arm64) and Windows (x64).`);
  console.error(`To build from source on this platform, run: cargo install tincan-chat`);
  process.exit(1);
}

// Windows releases ship a .zip; everything else a .tar.gz.
const archiveExt = isWindows ? 'zip' : 'tar.gz';
const binaryName = isWindows ? 'tincan.exe' : 'tincan';

function getCacheDir() {
  let base;
  if (process.env.XDG_CACHE_HOME) {
    base = process.env.XDG_CACHE_HOME;
  } else if (isWindows) {
    // ~/.cache is not a place Windows keeps anything; LOCALAPPDATA is.
    base = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
  } else if (process.platform === 'darwin') {
    base = path.join(os.homedir(), 'Library', 'Caches');
  } else {
    base = path.join(os.homedir(), '.cache');
  }
  const dir = path.join(base, 'tincan', 'bin', `v${VERSION}-${target}`);
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

const cacheDir = getCacheDir();
const binaryPath = path.join(cacheDir, binaryName);

// Every release asset ships with a `<asset>.sha256` beside it, written by the
// release workflow as `<hex digest>  <file name>`.
function fetchText(url) {
  return new Promise((resolve, reject) => {
    https.get(url, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        return fetchText(res.headers.location).then(resolve, reject);
      }
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode} from ${url}`));
      }
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (chunk) => { body += chunk; });
      res.on('end', () => resolve(body));
    }).on('error', reject);
  });
}

function sha256File(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function downloadAndExtract(url, destBinary, expectedSha) {
  return new Promise((resolve, reject) => {
    const tempArchive = path.join(cacheDir, `download-${Date.now()}.${archiveExt}`);
    const file = fs.createWriteStream(tempArchive);

    function fetch(currentUrl) {
      https.get(currentUrl, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return fetch(res.headers.location);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`Failed to download binary: HTTP ${res.statusCode} from ${currentUrl}`));
        }

        res.pipe(file);
        file.on('finish', () => {
          file.close(() => {
            try {
              const actualSha = sha256File(tempArchive);
              if (actualSha !== expectedSha) {
                throw new Error(`Checksum mismatch for ${url}: expected ${expectedSha}, got ${actualSha}`);
              }
              const extractDir = path.join(cacheDir, `extract-${Date.now()}`);
              fs.mkdirSync(extractDir, { recursive: true });
              if (isWindows) {
                // Windows 10 1803 and later ship bsdtar as `tar`, so this could
                // have stayed one line. Expand-Archive is used instead because
                // it is present on older builds too and this package promises
                // nothing about the Windows version, only about Node 16.
                execSync(
                  `powershell -NoProfile -NonInteractive -Command "Expand-Archive -LiteralPath '${tempArchive}' -DestinationPath '${extractDir}' -Force"`
                );
              } else {
                execSync(`tar -xzf "${tempArchive}" -C "${extractDir}"`);
              }
              const extractedBinary = path.join(extractDir, binaryName);
              if (!fs.existsSync(extractedBinary)) {
                throw new Error(`Archive did not contain ${binaryName}`);
              }
              fs.copyFileSync(extractedBinary, destBinary);
              // Windows decides what is runnable from the extension, and has no
              // permission bit to set.
              if (!isWindows) {
                fs.chmodSync(destBinary, 0o755);
              }
              fs.rmSync(extractDir, { recursive: true, force: true });
              if (fs.existsSync(tempArchive)) fs.unlinkSync(tempArchive);
              resolve();
            } catch (err) {
              if (fs.existsSync(tempArchive)) fs.unlinkSync(tempArchive);
              reject(err);
            }
          });
        });
      }).on('error', (err) => {
        if (fs.existsSync(tempArchive)) fs.unlinkSync(tempArchive);
        reject(err);
      });
    }

    fetch(url);
  });
}

async function main() {
  if (!fs.existsSync(binaryPath)) {
    const url = `https://github.com/bilalyazicioglu/tincan-cli/releases/download/v${VERSION}/tincan-${target}.${archiveExt}`;
    process.stderr.write(`\x1b[36m[tincan]\x1b[0m Downloading tincan v${VERSION} for ${key}...\n`);
    try {
      const expectedSha = (await fetchText(`${url}.sha256`)).trim().split(/\s+/)[0].toLowerCase();
      if (!/^[0-9a-f]{64}$/.test(expectedSha)) {
        throw new Error(`Malformed checksum file at ${url}.sha256`);
      }
      await downloadAndExtract(url, binaryPath, expectedSha);
    } catch (err) {
      console.error(`\x1b[31m[tincan]\x1b[0m Download failed: ${err.message}`);
      process.exit(1);
    }
  }

  const result = spawnSync(binaryPath, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) {
    console.error(result.error);
    process.exit(1);
  }
  process.exit(result.status ?? 0);
}

main();
