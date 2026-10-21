// Run only through browser-capability.mjs with its lock-matched core package.
const { createRequire } = require('node:module');
const path = require('node:path');
const api = createRequire(path.join(process.argv[2], 'package.json'))('./');
(async () => {
  let browser;
  try {
    browser = await api[process.argv[3]].launch({ headless: true, timeout: 30000 });
    const page = await browser.newPage();
    await page.setContent('<title>Coven browser readiness</title><main>ready</main>');
    if (await page.title() !== 'Coven browser readiness') throw new Error('Readiness title mismatch');
  } finally { if (browser) await browser.close(); }
})().catch(error => { console.error(String(error)); process.exitCode = 1; });
