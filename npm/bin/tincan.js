#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const os = require('os');
const https = require('https');
const { spawnSync, execSync } = require('child_process');

const pkg = require('../package.json');
const VERSION = pkg.version;

const PLATFORMS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
};

const key = `${process.platform}-${process.arch}`;
const target = PLATFORMS[key];

if (!target) {
  console.error(`\x1b[31m[tincan]\x1b[0m Unsupported platform: ${key}.`);
  console.error(`Prebuilt binaries are currently available for Apple Silicon macOS (arm64) and Linux (x86_64).`);
  console.error(`To build from source on this platform, run: cargo install tincan`);
  process.exit(1);
}

function getCacheDir() {
  const base = process.env.XDG_CACHE_HOME
    || (process.platform === 'darwin'
        ? path.join(os.homedir(), 'Library', 'Caches')
        : path.join(os.homedir(), '.cache'));
  const dir = path.join(base, 'tincan', 'bin', `v${VERSION}-${target}`);
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

const cacheDir = getCacheDir();
const binaryPath = path.join(cacheDir, 'tincan');

function downloadAndExtract(url, destBinary) {
  return new Promise((resolve, reject) => {
    const tempArchive = path.join(cacheDir, `download-${Date.now()}.tar.gz`);
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
              const extractDir = path.join(cacheDir, `extract-${Date.now()}`);
              fs.mkdirSync(extractDir, { recursive: true });
              execSync(`tar -xzf "${tempArchive}" -C "${extractDir}"`);
              const extractedBinary = path.join(extractDir, 'tincan');
              if (!fs.existsSync(extractedBinary)) {
                throw new Error('Archive did not contain tincan binary');
              }
              fs.copyFileSync(extractedBinary, destBinary);
              fs.chmodSync(destBinary, 0o755);
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
    const url = `https://github.com/bilalyazicioglu/tincan-cli/releases/download/v${VERSION}/tincan-${target}.tar.gz`;
    process.stderr.write(`\x1b[36m[tincan]\x1b[0m Downloading tincan v${VERSION} for ${key}...\n`);
    try {
      await downloadAndExtract(url, binaryPath);
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
