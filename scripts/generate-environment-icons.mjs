// Rasterize the existing vector brand with DEV/TEST badges; no brand redraw.
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(path.resolve(root, '../wt-media-cloud/web/package.json'));
const { chromium } = require('playwright');
const browser = await chromium.launch({ headless: true, channel: 'chrome' });
try {
  for (const env of ['dev', 'test']) {
    const page = await browser.newPage({ viewport: { width: 512, height: 512 }, deviceScaleFactor: 1 });
    const folder = path.join(root, 'src-tauri/icons', env);
    const svg = await readFile(path.join(folder, 'brand-mark.svg'), 'utf8');
    await page.setContent(`<style>html,body{margin:0;background:transparent}svg{width:512px;height:512px}</style>${svg}`);
    await page.locator('svg').screenshot({ path: path.join(folder, 'icon.png'), omitBackground: true });
    await page.close();
  }
} finally { await browser.close(); }
