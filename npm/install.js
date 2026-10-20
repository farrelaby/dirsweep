#!/usr/bin/env node
const { existsSync, createWriteStream, createReadStream, mkdirSync, chmodSync, writeFileSync, readFileSync, unlinkSync } = require("fs");
const { join } = require("path");
const { pipeline } = require("stream");
const { promisify } = require("util");
const https = require("https");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const pipe = promisify(pipeline);

const PKG = require("./package.json");
const VERSION = PKG.version;
const BIN_DIR = join(__dirname, "bin");
const BIN_PATH = join(BIN_DIR, process.platform === "win32" ? "dirsweep.exe" : "dirsweep");
const VERSION_FILE = join(BIN_DIR, "VERSION");
const TIMEOUT_MS = 30_000;

function platform() {
  const os = process.platform;
  const arch = process.arch;

  const map = {
    "win32-x64": "x86_64-pc-windows-msvc",
    "win32-arm64": "aarch64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "darwin-arm64": "aarch64-apple-darwin",
  };

  const key = `${os}-${arch}`;
  if (!map[key]) {
    console.error(`Unsupported platform: ${key}`);
    console.error(`dirsweep supports: ${Object.keys(map).join(", ")}`);
    process.exit(1);
  }
  return map[key];
}

function isWindows() {
  return process.platform === "win32";
}

function releaseUrl(target) {
  const ext = isWindows() ? "zip" : "tar.gz";
  return `https://github.com/farrelaby/dirsweep/releases/download/v${VERSION}/dirsweep-v${VERSION}-${target}.${ext}`;
}

function download(url, dest, redirects = 0) {
  if (!url.startsWith("https://")) {
    return Promise.reject(new Error("Insecure protocol: HTTPS required"));
  }
  const MAX_REDIRECTS = 10;
  return new Promise((resolve, reject) => {
    const req = https.get(url, { timeout: TIMEOUT_MS }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        if (redirects >= MAX_REDIRECTS) {
          reject(new Error(`Too many redirects (${MAX_REDIRECTS}): ${url}`));
          return;
        }
        const location = res.headers.location;
        if (!location.startsWith("https://")) {
          reject(new Error("Insecure protocol: HTTPS required"));
          return;
        }
        download(location, dest, redirects + 1).then(resolve).catch(reject);
        return;
      }
      if (res.statusCode !== 200) {
        reject(new Error(`Download failed with status ${res.statusCode}: ${url}`));
        return;
      }
      const file = createWriteStream(dest);
      pipe(res, file).then(resolve).catch(reject);
    });
    req.on("timeout", () => {
      req.destroy();
      reject(new Error(`Download timed out after ${TIMEOUT_MS}ms: ${url}`));
    });
    req.on("error", reject);
  });
}

function computeSha256(filePath) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash("sha256");
    const stream = createReadStream(filePath);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("end", () => resolve(hash.digest("hex").toLowerCase()));
    stream.on("error", reject);
  });
}

function parseChecksums(content) {
  const map = {};
  for (const line of content.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const match = trimmed.match(/^([a-fA-F0-9]{64})\s+\*?(.+)$/);
    if (match) {
      const filename = match[2].trim().replace(/^\.\//, "");
      map[filename] = match[1].toLowerCase();
    }
  }
  return map;
}

function isUpToDate() {
  if (!existsSync(VERSION_FILE)) return false;
  try {
    return readFileSync(VERSION_FILE, "utf8").trim() === VERSION;
  } catch {
    return false;
  }
}

async function install() {
  if (isUpToDate() && existsSync(BIN_PATH)) {
    return;
  }

  mkdirSync(BIN_DIR, { recursive: true });

  // Remove stale binary
  try { unlinkSync(BIN_PATH); } catch {}

  const target = platform();
  const ext = isWindows() ? "zip" : "tar.gz";
  const archiveFilename = `dirsweep-v${VERSION}-${target}.${ext}`;
  const url = releaseUrl(target);
  const archivePath = join(BIN_DIR, isWindows() ? "dirsweep.zip" : "dirsweep.tar.gz");
  const checksumsUrl = `https://github.com/farrelaby/dirsweep/releases/download/v${VERSION}/checksums.txt`;
  const checksumsPath = join(BIN_DIR, "checksums.txt");

  try {
    await download(checksumsUrl, checksumsPath);
  } catch (err) {
    console.error(`Failed to download checksums from ${checksumsUrl}`);
    console.error(err.message);
    process.exit(1);
  }

  const checksumsContent = readFileSync(checksumsPath, "utf8");
  const checksums = parseChecksums(checksumsContent);
  const expectedHash = checksums[archiveFilename];
  if (!expectedHash) {
    console.error(`Checksum not found for ${archiveFilename} in checksums.txt`);
    try { unlinkSync(checksumsPath); } catch {}
    process.exit(1);
  }

  console.log(`Downloading dirsweep v${VERSION} for ${target}...`);

  try {
    await download(url, archivePath);
  } catch (err) {
    console.error(`Failed to download from ${url}`);
    console.error(err.message);
    try { unlinkSync(checksumsPath); } catch {}
    process.exit(1);
  }

  const actualHash = await computeSha256(archivePath);
  if (actualHash !== expectedHash.toLowerCase()) {
    console.error(`Checksum mismatch for ${archiveFilename}: expected ${expectedHash}, got ${actualHash}`);
    try { unlinkSync(archivePath); } catch {}
    try { unlinkSync(checksumsPath); } catch {}
    process.exit(1);
  }

  try { unlinkSync(checksumsPath); } catch {}

  if (isWindows()) {
    const safeArchivePath = archivePath.replace(/'/g, "''");
    const safeBinDir = BIN_DIR.replace(/'/g, "''");
    const ps = spawnSync(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        `Expand-Archive -LiteralPath '${safeArchivePath}' -DestinationPath '${safeBinDir}' -Force`,
      ],
      { stdio: "inherit" }
    );
    if (ps.status !== 0) {
      console.error("Failed to extract archive");
      process.exit(1);
    }
  } else {
    const tar = spawnSync("tar", ["-xzf", archivePath, "-C", BIN_DIR], {
      stdio: "inherit",
    });
    if (tar.status !== 0) {
      console.error("Failed to extract archive");
      process.exit(1);
    }
  }

  try { unlinkSync(archivePath); } catch {}

  chmodSync(BIN_PATH, 0o755);
  writeFileSync(VERSION_FILE, VERSION);
  console.log(`dirsweep v${VERSION} installed at ${BIN_PATH}`);
}

install().catch((err) => {
  console.error(err);
  process.exit(1);
});
