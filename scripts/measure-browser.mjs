import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as pause } from 'node:timers/promises';
import { openChromium } from './chromium.mjs';

const [out, base = 'https://keysend.app', count = '3', mode = 'browser'] = process.argv.slice(2);
if (!out) throw new Error('Usage: node scripts/measure-browser.mjs OUTPUT [BASE] [TRIALS]');
const pages = ['/', '/sign-in', '/privacy'];
const privatePages = ['/ad-accounts', '/subscription', '/settings'];
const session = process.env.KEYSEND_SESSION;
if (session && !['127.0.0.1', 'localhost'].includes(new URL(base).hostname)) throw new Error('KEYSEND_SESSION is for a local seeded server only');
const trials = Number(count);
if (!Number.isInteger(trials) || trials < 1 || trials > 10) throw new Error('TRIALS must be 1..10');
await mkdir(out, { recursive: true, mode: 0o700 });
const browser = await openChromium(join(out, 'chromium.log'));
const { executable, call, evaluate, handlers } = browser;
const cleanURL = value => {
  try {
    const url = new URL(value);
    for (const key of [...url.searchParams.keys()]) if (key !== 'v') url.searchParams.set(key, '[redacted]');
    return url.href;
  } catch { return value; }
};
const safeHeaders = headers => Object.fromEntries(Object.entries(headers).filter(([k]) =>
  ['cache-control', 'cf-cache-status', 'cf-ray', 'age', 'content-encoding', 'content-type', 'x-keysend-release'].includes(k.toLowerCase())));
const observer = `window.__perf = {lcp: [], shifts: [], tasks: []};
  new PerformanceObserver(list => { for (const e of list.getEntries()) window.__perf.lcp.push({time:e.startTime,size:e.size,element:e.element?.tagName,id:e.element?.id,url:e.url}); }).observe({type:'largest-contentful-paint',buffered:true});
  new PerformanceObserver(list => { for (const e of list.getEntries()) if (!e.hadRecentInput) window.__perf.shifts.push({time:e.startTime,value:e.value}); }).observe({type:'layout-shift',buffered:true});
  new PerformanceObserver(list => { for (const e of list.getEntries()) window.__perf.tasks.push({time:e.startTime,duration:e.duration}); }).observe({type:'longtask',buffered:true});`;
const stateExpression = `(() => {
  const n = performance.getEntriesByType('navigation')[0];
  const resources = performance.getEntriesByType('resource').map(e => ({url:e.name,type:e.initiatorType,start:e.startTime,end:e.responseEnd,transfer:e.transferSize,encoded:e.encodedBodySize,decoded:e.decodedBodySize,blocking:e.renderBlockingStatus}));
  const p = window.__perf;
  let cls=0, score=0, start=0, last=0;
  for (const e of p.shifts) { if(e.time-last>1000 || e.time-start>5000){score=0;start=e.time;} score+=e.value;last=e.time;cls=Math.max(cls,score); }
  const fcp = performance.getEntriesByName('first-contentful-paint')[0]?.startTime;
  return {title:document.title,status:n.responseStatus,ttfb_ms:n.responseStart,document_complete_ms:n.responseEnd,load_ms:n.loadEventEnd,
    fcp_ms:fcp,lcp:p.lcp.at(-1),cls,tbt_lab_ms:p.tasks.filter(e=>e.time+e.duration>fcp).reduce((sum,e)=>sum+Math.max(0,e.duration-50),0),
    font_status:document.fonts.status,resources,images:[...document.images].map(i=>({url:i.currentSrc,width:i.width,height:i.height,naturalWidth:i.naturalWidth,naturalHeight:i.naturalHeight,loading:i.loading,complete:i.complete,top:i.getBoundingClientRect().top,widthAttribute:i.getAttribute('width'),heightAttribute:i.getAttribute('height')})),
    scripts:[...document.scripts].map(s=>({url:s.src,defer:s.defer,async:s.async})),
    font_preloads:[...document.querySelectorAll('link[rel=preload][as=font]')].map(x=>x.href),
    document_transfer:n.transferSize,document_encoded:n.encodedBodySize,document_decoded:n.decodedBodySize};
})()`;
let requests = new Map();
handlers.set('Network.requestWillBeSent', e => requests.set(e.requestId, {url:cleanURL(e.request.url),type:e.type,start:e.timestamp}));
handlers.set('Network.responseReceived', e => {
  const request = requests.get(e.requestId);
  if (request) Object.assign(request, {status:e.response.status,protocol:e.response.protocol,
    headers:safeHeaders(e.response.headers),fromDiskCache:e.response.fromDiskCache,fromServiceWorker:e.response.fromServiceWorker});
});
handlers.set('Network.loadingFinished', e => {
  const request = requests.get(e.requestId);
  if (request) Object.assign(request, {wire_bytes:e.encodedDataLength,end:e.timestamp});
});
handlers.set('Network.loadingFailed', e => {
  const request = requests.get(e.requestId);
  if (request) Object.assign(request, {error:e.errorText,canceled:e.canceled});
});
try {
  await call('Network.enable');
  if (mode === 'regions') {
    const results = {tool:'Uptrends public full-page Chrome test, desktop 1600x900, native connection',
      started_at:new Date().toISOString(),rows:[]};
    for (const region of ['Frankfurt','New York','Singapore']) {
      for (const path of pages) {
        await call('Page.navigate',{url:'https://app.uptrends.com/tools/fpc?lang=en'});
        let deadline=Date.now()+30000;
        while (Date.now()<deadline && !await evaluate("!!document.querySelector('input[name=tools_start_form_address]') && !!document.querySelector('button.start-button') && !document.querySelector('button.start-button').disabled")) await pause(200);
        await evaluate(`(() => {
          const input=document.querySelector('input[name=tools_start_form_address]');
          if (!input) throw new Error('Uptrends URL field unavailable');
          input.value=${JSON.stringify(base+path)};
          input.dispatchEvent(new Event('input',{bubbles:true}));
          input.dispatchEvent(new Event('change',{bubbles:true}));
          const select=[...document.querySelectorAll('select')].find(e=>[...e.options].some(o=>o.label===${JSON.stringify(region)}));
          if (!select) throw new Error('Uptrends region unavailable');
          select.value=[...select.options].find(o=>o.label===${JSON.stringify(region)}).value;
          select.dispatchEvent(new Event('change',{bubbles:true}));
        })()`);
        await pause(200);
        await evaluate("document.querySelector('button.start-button').click()");
        deadline=Date.now()+90000;
        let summary;
        while (Date.now()<deadline) {
          summary=await evaluate("document.body.innerText.split('General information')[1]?.split('Filmstrip')[0] ?? null");
          if (summary) break;
          await pause(1000);
        }
        if (!summary) {
          await writeFile(join(out,'uptrends-error.txt'), await evaluate('document.body.innerText.slice(0,6000)'), {mode:0o600});
          throw new Error(`Uptrends failed or timed out: ${region} ${path}; see uptrends-error.txt`);
        }
        const field = label => summary.split(label+'\n')[1]?.split('\n')[0]?.trim();
        const row={path,region,observed_at:new Date().toISOString(),summary,
          load:field('LOAD TIME'),ttfb:field('Time to first byte'),load_event:field('Load event'),
          lcp:field('Largest contentful paint'),cls:field('Cumulative layout shift'),tbt:field('Total blocking time'),
          bytes:field('SIZE'),requests:field('REQUESTS'),location:field('LOCATION'),browser:field('BROWSER')};
        results.rows.push(row);
        await writeFile(join(out,'regions.json'),JSON.stringify(results,null,2)+'\n',{mode:0o600});
        console.log(JSON.stringify(row));
      }
    }
  } else {
  await call('Emulation.setDeviceMetricsOverride', {width:412,height:823,deviceScaleFactor:1.75,mobile:true});
  await call('Emulation.setTouchEmulationEnabled', {enabled:true});
  await call('Network.setUserAgentOverride', {userAgent:'Mozilla/5.0 (Linux; Android 11; Moto G Power) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Mobile Safari/537.36'});
  await call('Network.emulateNetworkConditions', {offline:false,latency:150,downloadThroughput:1600000/8,uploadThroughput:750000/8,connectionType:'cellular4g'});
  await call('Emulation.setCPUThrottlingRate', {rate:4});
  await call('Page.addScriptToEvaluateOnNewDocument', {source:observer});
  const results = {tool:'Chromium CDP, directly observed, not Lighthouse simulation or field CWV',
    profile:{width:412,height:823,dpr:1.75,latency_ms:150,download_bps:1600000,upload_bps:750000,cpu_slowdown:4},
    started_at:new Date().toISOString(),executable,rows:[]};
  const collect = async () => {
    const row = await evaluate(stateExpression);
    row.resources.forEach(r => {r.url=cleanURL(r.url);});
    row.images.forEach(r => {r.url=cleanURL(r.url);});
    row.scripts.forEach(r => {r.url=cleanURL(r.url);});
    if (row.lcp?.url) row.lcp.url=cleanURL(row.lcp.url);
    row.requests = [...requests.values()];
    row.request_count = row.requests.length;
    row.wire_bytes = row.requests.reduce((sum,r)=>sum+(r.wire_bytes??0),0);
    return row;
  };
  const pagePaths = process.env.KEYSEND_MEASURE_PATHS?.split(',') ?? pages;
  if (pagePaths.some(path => !(session ? [...pages, ...privatePages] : pages).includes(path))) throw new Error('Invalid measurement path');
  if (session) await call('Network.setCookie', {name:'ks_session', value:session, url:base, httpOnly:true});
  for (const path of pagePaths) {
    for (let trial = 0; trial < trials + 1; trial++) {
      const warm = mode === 'repeat' ? trial > 0 : trial === trials;
      await call('Page.navigate', {url:'about:blank'});
      await pause(100);
      requests = new Map();
      await call('Network.setCacheDisabled', {cacheDisabled:false});
      if (!warm) await call('Network.clearBrowserCache');
      await call('Page.navigate', {url:base+path});
      const deadline = Date.now()+35000;
      while (Date.now()<deadline) {
        if (await evaluate('document.readyState === "complete" && performance.getEntriesByType("navigation")[0]?.loadEventEnd > 0')) break;
        await pause(100);
      }
      if (Date.now()>=deadline) throw new Error(`Load timeout: ${path}`);
      await pause(5000);
      if (session && path !== '/sign-in' && await evaluate('location.pathname') === '/sign-in') throw new Error(`Session rejected: ${path}`);
      const initial = await collect();
      const row = {path,trial,cache:warm?'warm-browser':'cold-browser',initial};
      if (trial === 0) {
        const screenshot = await call('Page.captureScreenshot', {format:'png',captureBeyondViewport:false});
        const filename = join(out, `${path.replaceAll('/','-').replace(/^-|-$/g,'')||'home'}-phone.png`);
        await writeFile(filename, Buffer.from(screenshot.data,'base64'), {mode:0o600});
        row.screenshot=filename;
        await evaluate(`(async()=>{for(let y=0;y<document.documentElement.scrollHeight;y+=window.innerHeight){window.scrollTo(0,y);await new Promise(r=>setTimeout(r,150));}})()`);
        await pause(5000);
        row.scrolled = await collect();
      }
      results.rows.push(row);
      await writeFile(join(out,'browser.json'),JSON.stringify(results,null,2)+'\n',{mode:0o600});
      console.log(JSON.stringify({path,trial,cache:row.cache,ttfb_ms:initial.ttfb_ms,load_ms:initial.load_ms,
        lcp_ms:initial.lcp?.time,cls:initial.cls,tbt_ms:initial.tbt_lab_ms,bytes:initial.wire_bytes,requests:initial.request_count}));
    }
  }
  }
} finally {
  await browser.close();
}
