// Development-only contract check against a user-provided original worker.
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const original = await readFile(process.argv[2] || 'artifacts/io_worker.js', 'utf8');
const patch = await readFile(new URL('../crates/wgr-http/src/ram_io.js', import.meta.url), 'utf8');
async function run(adapted) {
  const calls = [], data = Uint8Array.from({ length: 1048576 }, (_, i) => i & 255);
  const sandbox = vm.createContext({
    self: { postMessage() {} }, Uint8Array, Int32Array, ArrayBuffer, SharedArrayBuffer, Atomics,
    Map, Set, Promise, TextDecoder, TextEncoder, performance, setTimeout, clearTimeout,
    console, URL, Number, Math, Error,
    navigator: { storage: { getDirectory() { throw Error('OPFS não autorizado neste teste'); } } },
    BroadcastChannel: class { postMessage() {} },
    fetch: async (url, init) => {
      if (url.includes('manifest.json')) return { status: 200, ok: true, json: async () => ({ version: 1, files: [['header.rpf', data.length]] }) };
      if (url.includes('bootset')) return { status: 200, ok: true, json: async () => [['header.rpf', [[0, 4095]]]] };
      const [, a, z] = /bytes=(\d+)-(\d+)/.exec(init.headers.Range);
      calls.push([Number(a), Number(z)]);
      return new Response(data.slice(Number(a), Number(z) + 1), { status: 206 });
    }
  });
  vm.runInContext(original + (adapted ? '\n' + patch : ''), sandbox);
  await vm.runInContext("init({base:'http://fixture/data/',noStore:true,bootset:'bootset_low.json'})", sandbox);
  await new Promise(resolve => setTimeout(resolve, 5));
  const length = await vm.runInContext("mem={buffer:new SharedArrayBuffer(65536)};HEAPU8=new Uint8Array(mem.buffer);HEAP32=new Int32Array(mem.buffer);readWithoutStore(0,0,16,0)", sandbox);
  assert.equal(length, 16);
  assert.deepEqual([...vm.runInContext('HEAPU8.subarray(0,16)', sandbox)], Array.from({ length: 16 }, (_, i) => i));
  const header = { requests: calls.length, bytes: calls.reduce((n, [a, z]) => n + z - a + 1, 0) };
  const large = await vm.runInContext("mem={buffer:new SharedArrayBuffer(2097152)};HEAPU8=new Uint8Array(mem.buffer);HEAP32=new Int32Array(mem.buffer);readWithoutStore(0,333,700000,4096)", sandbox);
  assert.equal(large,700000);
  assert.deepEqual([...vm.runInContext('HEAPU8.subarray(4096,4096+700000)',sandbox)],Array.from({length:700000},(_,i)=>(333+i)&255));
  return header;
}
const originalResult = await run(false), adaptedResult = await run(true);
assert.equal(originalResult.bytes, 262144);
assert.equal(adaptedResult.bytes, 4096);
console.log(JSON.stringify({ passed: true, original: originalResult, adapted: adaptedResult, opfs_used: false, real_worker_contract: true }));
