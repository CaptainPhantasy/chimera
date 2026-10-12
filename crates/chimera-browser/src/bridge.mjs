// Puppeteer is supplied by the operator. Chrome uses pipes, not a listening port.
import {createInterface} from 'node:readline';
import {pathToFileURL} from 'node:url';
import path from 'node:path';
let browser, launch, page, config;
const messages = [];
const lines = createInterface({input: process.stdin, crlfDelay: Infinity});
const now = () => new Date().toISOString();
const clip = value => String(value).slice(0, 100000);
function permitted(url) {
  const parsed = new URL(url);
  return parsed.protocol === 'about:' || parsed.protocol === 'data:' ||
    (['http:', 'https:'].includes(parsed.protocol) && config.allowed_origins.includes(parsed.origin));
}
async function close() {
  if (browser) await browser.close();
  else if (launch) { try { await (await launch).close(); } catch {} }
}
process.on('SIGTERM', () => close().finally(() => process.exit(0)));
process.on('SIGHUP', () => close().finally(() => process.exit(0)));
lines.on('close', () => close().finally(() => process.exit(0)));
for await (const line of lines) {
  let reply;
  try {
    if (line.length > 200000) throw new Error('browser request exceeds size limit');
    const request = JSON.parse(line);
    if (request.op === 'start') {
      config = request.config;
      const module = await import(pathToFileURL(config.automation_module).href);
      const puppeteer = module.default || module;
      launch = puppeteer.launch({executablePath: config.executable_path, headless: true,
        pipe: true, timeout: 15000, protocolTimeout: 30000,
        handleSIGTERM: false, handleSIGHUP: false, handleSIGINT: false,
        args: ['--disable-background-networking', '--disable-sync', '--disable-default-apps', '--no-first-run']});
      browser = await launch;
      page = await browser.newPage();
      await page.setViewport({width: 1440, height: 900, deviceScaleFactor: 1});
      if (config.allowed_origins.length === 0) await page.setOfflineMode(true);
      page.setDefaultTimeout(15000);
      page.setDefaultNavigationTimeout(15000);
      await page.setRequestInterception(true);
      page.on('request', request => {
        if (request.isInterceptResolutionHandled()) return;
        if (permitted(request.url())) request.continue().catch(() => {});
        else request.abort('blockedbyclient').catch(() => {});
      });
      page.on('console', msg => { messages.push({level: msg.type(), text: msg.text().slice(0, 2000), at: now()}); if (messages.length > 100) messages.shift(); });
      page.on('pageerror', error => { messages.push({level: 'error', text: String(error).slice(0, 2000), at: now()}); if (messages.length > 100) messages.shift(); });
      reply = {browser_pid: browser.process().pid};
    } else if (request.op === 'state') {
      reply = {current_url: page.url(), title: await page.title(), active: browser.connected && !page.isClosed()};
    } else if (request.op === 'close') {
      await close(); reply = {closed: true};
    } else if (request.op === 'act') {
      const action = request.action;
      reply = {type: 'ack'};
      switch (action.action) {
        case 'navigate':
          if (!permitted(action.url)) throw new Error('URL origin is not approved for this browser');
          await page.goto(action.url, {waitUntil: 'domcontentloaded'}); break;
        case 'click': await page.locator(action.selector).click(); break;
        case 'type': await page.locator(action.selector).fill(action.text); break;
        case 'wait_for': await page.waitForSelector(action.selector, {timeout: Math.min(30000, Math.max(1, action.timeout_ms))}); break;
        case 'eval_js': {
          const value = await page.evaluate(action.script);
          if (JSON.stringify(value ?? null).length > 100000) throw new Error('JavaScript result exceeds size limit');
          reply = {type: 'js_result', value: value ?? null}; break;
        }
        case 'scroll': await page.evaluate(({x,y}) => window.scrollBy(x,y), action); break;
        case 'screenshot': {
          const target = path.resolve(request.screenshot_path);
          if (path.dirname(target) !== path.resolve(config.artifact_dir) || !target.endsWith('.png')) throw new Error('screenshot destination is outside artifact storage');
          await page.screenshot({path: target, type: 'png', fullPage: false});
          reply = {type: 'screenshot', uri: target, captured_at: now()}; break;
        }
        case 'read_text': {
          const content = action.selector ? await page.$eval(action.selector, el => el.innerText ?? el.textContent) : await page.evaluate(() => document.body?.innerText ?? '');
          reply = {type: 'text', content: clip(content)}; break;
        }
        case 'read_console': reply = {type: 'console', messages: [...messages]}; break;
        default: throw new Error(`unsupported browser action: ${action.action}`);
      }
    } else throw new Error(`unknown bridge operation: ${request.op}`);
    process.stdout.write(JSON.stringify({ok: true, value: reply}) + '\n');
  } catch (error) {
    process.stdout.write(JSON.stringify({ok: false, error: String(error).slice(0, 4000)}) + '\n');
  }
}
