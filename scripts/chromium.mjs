import { spawn } from 'node:child_process';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { createWriteStream, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as pause } from 'node:timers/promises';

async function findExecutable() {
  if (process.env.KEYSEND_CHROMIUM) return process.env.KEYSEND_CHROMIUM;
  const cache = join(process.env.HOME ?? '', 'Library/Caches/ms-playwright');
  const shells = (await readdir(cache).catch(() => []))
    .filter(name => name.startsWith('chromium_headless_shell-'))
    .sort((a, b) => Number(b.split('-')[1]) - Number(a.split('-')[1]))
    .map(name => join(cache, name, 'chrome-headless-shell-mac-arm64/chrome-headless-shell'));
  const found = [...shells, '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/usr/bin/google-chrome', '/usr/bin/chromium'].find(existsSync);
  if (!found) throw new Error('No Chromium was found. Set KEYSEND_CHROMIUM to an executable.');
  return found;
}

export async function openChromium(logFile) {
  const executable = await findExecutable();
  const profile = await mkdtemp(join(tmpdir(), 'keysend-chrome-'));
  const log = createWriteStream(logFile, { mode: 0o600 });
  const chrome = spawn(executable, ['--headless', '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1',
    `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: ['ignore', 'pipe', 'pipe'] });
  chrome.stdout.pipe(log);
  chrome.stderr.pipe(log);
  let socket;
  let id = 0;
  const pending = new Map();
  const handlers = new Map();
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const requestId = ++id;
    const timer = setTimeout(() => { pending.delete(requestId); reject(new Error(`CDP timeout: ${method}`)); }, 30000);
    pending.set(requestId, { resolve, reject, timer });
    socket.send(JSON.stringify({ id: requestId, method, params }));
  });
  const evaluate = async expression => {
    const answer = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (answer.exceptionDetails) throw new Error(answer.exceptionDetails.text);
    return answer.result.value;
  };
  const close = async () => {
    socket?.close();
    if (chrome.exitCode === null) {
      const exited = new Promise(resolve => chrome.once('exit', resolve));
      chrome.kill('SIGTERM');
      await Promise.race([exited, pause(5000)]);
      if (chrome.exitCode === null) { chrome.kill('SIGKILL'); await exited; }
    }
    await rm(profile, { recursive: true, force: true });
    log.end();
  };
  try {
    const failed = new Promise((_, reject) => chrome.once('error', reject));
    const ready = (async () => {
      for (let i = 0; i < 100; i++) {
        try { return Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); }
        catch { if (chrome.exitCode !== null) throw new Error(`Chromium exited ${chrome.exitCode}`); await pause(100); }
      }
      throw new Error('Chromium did not publish its CDP port');
    })();
    const port = await Promise.race([ready, failed]);
    const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    socket = new WebSocket(tabs.find(t => t.type === 'page').webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
    socket.onmessage = event => {
      const message = JSON.parse(event.data);
      if (message.id) {
        const p = pending.get(message.id);
        if (!p) return;
        pending.delete(message.id);
        clearTimeout(p.timer);
        message.error ? p.reject(new Error(JSON.stringify(message.error))) : p.resolve(message.result);
      } else handlers.get(message.method)?.(message.params);
    };
    await call('Page.enable');
    await call('Runtime.enable');
  } catch (error) {
    await close();
    throw error;
  }
  return { executable, call, evaluate, handlers, close };
}

export async function load({ call, evaluate }, url, timeout = 35000) {
  await call('Page.navigate', { url });
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate('document.readyState === "complete" && performance.getEntriesByType("navigation")[0]?.loadEventEnd > 0')) return;
    await pause(100);
  }
  throw new Error(`Load timeout: ${url}`);
}
