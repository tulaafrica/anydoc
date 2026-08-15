#!/usr/bin/env node
/**
 * Fetch the prebuilt iOS Rust core (AnydocCore.xcframework) from this
 * package's GitHub Release — the iOS twin of what gradle does for Android
 * automatically. Runs as the package's postinstall; consumers can also run
 * it by hand:
 *
 *   node node_modules/react-native-anydoc/scripts/install-anydoc-core.js
 *
 * Behavior:
 *   - already present            -> no-op
 *   - not macOS                  -> no-op (iOS builds only happen on macOS)
 *   - ANYDOC_SKIP_IOS_CORE=1     -> no-op (opt out, e.g. Android-only CI)
 *   - ANYDOC_IOS_CORE_URL=<url>  -> download from there instead (mirrors)
 *   - download/unzip failure     -> WARN and exit 0. Install must never
 *     break: Android needs none of this, and the manual path (download the
 *     zip from Releases, unzip into this package's ios/) still works.
 *
 * Uses curl + unzip (both ship with macOS) rather than JS deps: this file
 * must run with zero node_modules of its own.
 */
const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const root = path.join(__dirname, '..');
const dest = path.join(root, 'ios');
const marker = path.join(dest, 'AnydocCore.xcframework', 'Info.plist');

function log(msg) {
  console.log(`[react-native-anydoc] ${msg}`);
}

if (process.env.ANYDOC_SKIP_IOS_CORE === '1') {
  log('ANYDOC_SKIP_IOS_CORE=1 — skipping iOS core download.');
  process.exit(0);
}
if (fs.existsSync(marker)) {
  process.exit(0); // already installed
}
if (process.platform !== 'darwin') {
  process.exit(0); // iOS builds only happen on macOS; Android is gradle's job
}

const { version } = require(path.join(root, 'package.json'));
const url =
  process.env.ANYDOC_IOS_CORE_URL ||
  `https://github.com/tulaafrica/anydoc/releases/download/rn-v${version}/AnydocCore.xcframework-${version}.zip`;

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'anydoc-core-'));
const zip = path.join(tmp, 'AnydocCore.xcframework.zip');

try {
  log(`downloading iOS core (rn-v${version}) …`);
  execFileSync('curl', ['-fsSL', '--retry', '3', '-o', zip, url], { stdio: 'inherit' });
  fs.mkdirSync(dest, { recursive: true });
  execFileSync('unzip', ['-oq', zip, '-d', dest], { stdio: 'inherit' });
  if (!fs.existsSync(marker)) {
    throw new Error('archive did not contain AnydocCore.xcframework');
  }
  log('iOS core installed.');
} catch (err) {
  log(`WARNING: could not install the iOS core automatically (${err.message}).`);
  log(`Android builds are unaffected. For iOS, download ${url}`);
  log(`and unzip it into ${dest} — or re-run this script on a network that can reach GitHub.`);
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}
process.exit(0);
