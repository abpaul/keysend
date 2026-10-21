// Usage: [KEYSEND_SESSION=<cookie>] [KEYSEND_SESSION_NAME=ks_admin] node scripts/screenshot.mjs OUTPUT BASE PATH...
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { load, openChromium } from './chromium.mjs';

const [out, base, ...paths] = process.argv.slice(2);
if (!out || !base || paths.length === 0) throw new Error('Usage: node scripts/screenshot.mjs OUTPUT BASE PATH...');
const session = process.env.KEYSEND_SESSION;
if (session && !['127.0.0.1', 'localhost'].includes(new URL(base).hostname)) throw new Error('KEYSEND_SESSION is for a local seeded server only');
await mkdir(out, { recursive: true, mode: 0o700 });
const browser = await openChromium(join(out, 'chromium.log'));
const { call, evaluate } = browser;
let failed = false;
try {
  if (session) await call('Network.setCookie', { name: process.env.KEYSEND_SESSION_NAME || 'ks_session', value: session, url: base, httpOnly: true });
  for (const path of paths) {
    for (const width of [390, 1280]) {
      await call('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: width < 600 });
      await load(browser, base + path);
      const page = await evaluate(`({status: performance.getEntriesByType('navigation')[0].responseStatus, path: location.pathname,
        title: document.title, width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight})`);
      const shot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true,
        clip: { x: 0, y: 0, width, height: page.height, scale: 1 } });
      const file = join(out, `${path.replaceAll('/', '-').replace(/^-|-$/g, '') || 'home'}-${width}.png`);
      await writeFile(file, Buffer.from(shot.data, 'base64'), { mode: 0o600 });
      const row = { path, width, status: page.status, landed: page.path, title: page.title, overflow_px: Math.max(0, page.width - width), file };
      if (page.status >= 400 || page.path !== path.split('?')[0]) { row.failed = true; failed = true; }
      console.log(JSON.stringify(row));
    }
  }
} finally {
  await browser.close();
}
if (failed) process.exit(1);
