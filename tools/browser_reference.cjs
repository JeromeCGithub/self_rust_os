#!/usr/bin/env node
// Development-only reference capture. No host renderer is used by jbrowse.
const fs = require('node:fs');
const path = require('node:path');
const {pathToFileURL} = require('node:url');
const crypto = require('node:crypto');
async function main() {
  const [source, destination, snapshot, charset = 'UTF-8'] = process.argv.slice(2);
  if (!source || !destination) throw Error('usage: browser_reference.cjs FILE_OR_URL OUTPUT_DIR [SNAPSHOT_HTML [CHARSET]]');
  const {chromium} = require(process.env.RUSTOS_PLAYWRIGHT_CORE || 'playwright-core');
  const url = /^https?:\/\//.test(source) ? source : pathToFileURL(path.resolve(source)).href;
  const bytes = snapshot ? fs.readFileSync(snapshot) : url.startsWith('file:') ? fs.readFileSync(source) : undefined;
  const out = path.resolve(destination);
  fs.mkdirSync(out, {recursive: true});
  const browser = await chromium.launch({
    executablePath: process.env.RUSTOS_CHROMIUM || '/usr/bin/chromium',
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  });
  try {
    const context = await browser.newContext({viewport: {width: 640, height: 308}});
    const page = await context.newPage();
    if (snapshot) {
      if (!/^https?:\/\//.test(source)) throw Error('snapshot requires its original HTTP(S) URL');
      await page.route(source, route => route.fulfill({body: bytes, contentType: 'text/html; charset=' + charset}));
    }
    await page.goto(url, {waitUntil: 'networkidle', timeout: 30000});
    await page.screenshot({path: path.join(out, 'reference.png')});
    const result = await page.evaluate(() => ({
      title: document.title,
      viewport: [innerWidth, innerHeight],
      elements: [...document.querySelectorAll('[id]')].map(element => {
        const rect = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        return {id: element.id, tag: element.tagName,
          box: [rect.x, rect.y, rect.width, rect.height],
          font: style.font, color: style.color, display: style.display,
          margin: style.margin, padding: style.padding};
      }),
    }));
    result.browser = browser.version();
    result.source = source;
    if (bytes) result.source_sha256 = crypto.createHash('sha256').update(bytes).digest('hex');
    fs.writeFileSync(path.join(out, 'reference.json'), JSON.stringify(result, null, 2) + '\n');
    console.log(JSON.stringify(result, null, 2));
  } finally {
    await browser.close();
  }
}
main().catch(error => {console.error(error); process.exitCode = 1;});
