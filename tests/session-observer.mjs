import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const reports = [], timers = [], workers = [], fetches = [], channels=[];
let resolveModule;
const compiled = new Promise(resolve => { resolveModule = resolve; });
class Worker {
  constructor(url) { this.url = String(url); this.posts = []; this.listeners = {}; workers.push(this); }
  postMessage(message, transfers) { this.posts.push({ message, transfers }); }
  addEventListener(name, callback) { this.listeners[name] = callback; }
}
const self = { Worker }, module = {};
const context = { self, top: self, URL, URLSearchParams, Proxy, Reflect,
  location: new URL('https://fiveplay.lol/?mode=sandbox'),
  chrome: { webview: { postMessage: text => reports.push(JSON.parse(text)) } },
  WebAssembly: { compileStreaming: () => compiled },
  fetch: (url, options) => { fetches.push({ url: String(url), options }); return {}; },
  setInterval: (callback, ms) => { timers.push({ callback, ms }); },
  addEventListener() {}, BroadcastChannel: class {constructor(name){this.name=name;channels.push(this)}},
  document: { documentElement: { dataset: {} }, getElementById: () => null }
};
vm.runInNewContext(await readFile(new URL('../crates/wgr-app/src/session_observer.js', import.meta.url), 'utf8'), context);
const untouched = new self.Worker('/other.js');
assert.equal(untouched.url, '/other.js');
const worker = new self.Worker('/b/build/loader.js');
assert.match(worker.url, /wgr_loader=bounded-v2/);
const canvas = {}, transfers = [canvas], args = ['-frameLimit=0'];
worker.postMessage({ canvas, args, cores: 2, lowMemory: true }, transfers);
assert.equal(worker.posts.length, 0, 'GPU/IO must not start before compilation succeeds');
assert.equal(fetches.length, 1);
assert.equal(fetches[0].url, 'https://fiveplay.lol/b/build/game.wasm?wgr_native=1');
resolveModule(module); await Promise.resolve(); await Promise.resolve();
assert.equal(worker.posts.length, 1);
assert.equal(worker.posts[0].message.wgrModule, module);
assert.equal(worker.posts[0].message.canvas, canvas);
assert.equal(worker.posts[0].transfers, transfers, 'preserve the canvas transfer list');
assert(worker.posts[0].message.args.includes('-textureQuality=1'));
assert(worker.posts[0].message.args.includes('-lodScale=0.5'));
assert.equal(reports[0].payload.configuration.args[0], '-frameLimit=0');
worker.listeners.message({ data: { input: { memory: { buffer: { byteLength: 3221225472 } } } } });
const timer = timers.find(timer => timer.ms === 5000);
timer.callback(); timer.callback();
assert.equal(reports.filter(report => report.payload?.memory).length, 1, 'unchanged memory must not generate repeated IPC');
assert.equal(reports.at(-1).payload.memory.linear_bytes, 3221225472);
worker.postMessage({ event: 'other' });
assert.equal(worker.posts.length, 2);
context.location.search='?wgr_quality=economy';
const economy=new self.Worker('/b/build/loader.js');
economy.postMessage({canvas,args:['-textureQuality=0','-lodScale=0']},transfers);
await Promise.resolve();await Promise.resolve();
assert.deepEqual(economy.posts[0].message.args,['-textureQuality=0','-lodScale=0']);
context.location.search='?wgr_quality=balanced&args=-textureQuality=2';
const custom=new self.Worker('/b/build/loader.js');
custom.postMessage({canvas,args:['-textureQuality=0','-textureQuality=2','-lodScale=0']},transfers);
await Promise.resolve();await Promise.resolve();
assert.equal(custom.posts[0].message.args.filter(arg=>arg.startsWith('-textureQuality=')).join(),'-textureQuality=2');
const gpuLog=channels.find(channel=>channel.name==='game-log');
const before=reports.length;
gpuLog.onmessage({data:'key pressed: private engine log'});assert.equal(reports.length,before);
gpuLog.onmessage({data:'[wgpu] fps: 80'});assert.equal(reports.at(-1).payload.gpu_log,'[wgpu] fps: 80');
console.log(JSON.stringify({ passed: true, compilation_before_workers: true, original_transfers: true, memory_report_deduplicated: true }));
