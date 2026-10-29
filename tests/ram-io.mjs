import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const sandbox = { self: {}, Uint8Array, Map, Set, Promise, Error, Number, Math };
vm.runInNewContext(await readFile(new URL('../crates/wgr-http/src/ram_io.js', import.meta.url), 'utf8'), sandbox);
const Cache = sandbox.self.WgrRamCache;
const audioAhead=sandbox.self.wgrAudioAhead;
let audioState=audioAhead(null,0,33792,8000000);
assert.equal(audioState.ranges.length,0);
for(let i=1;i<12;i++){
  audioState=audioAhead(audioState,i*33792,33792,8000000);
  assert(audioState.ahead<=1048576&&audioState.ranges.length<=8);
  for(const [a,z] of audioState.ranges)assert(a%4096===0&&z-a<131072&&z<8000000);
}
assert.equal(audioState.ahead,1048576);
assert.equal(audioAhead(audioState,32,1024,8000000).ahead,0,'random seek resets speculation');
assert.equal(audioAhead({end:7900000,ahead:1048576},7900000,100000,8000000).ranges.length,0,'no prefetch past EOF');
const bootSet = [['a', [[0, 63], [4090, 9000], [8000, 300000], [-1, 10], [999999, 1000000]]], ['missing', [[0, 100]]], ['b', [[0, 7]]]];
const index = new Map([['a', 0], ['b', 1]]), size = id => id === 0 ? 300003 : 8;
const boot = [...sandbox.self.wgrBootRanges(bootSet, index, size, true)].map(r => [...r]);
assert.deepEqual(boot, [[0, 0, 4095], [1, 0, 7], [0, 4096, 135167], [0, 135168, 266239], [0, 266240, 300002]]);
assert.deepEqual([...sandbox.self.wgrBootRanges(bootSet, index, size, false)].map(r => [...r]), boot.slice(0, 2));
assert.equal([...sandbox.self.wgrBootRanges(null, index, size, true)].length, 0);
let calls = 0, bytes = 0, active = 0, peak = 0;
const order = [];
const expected = (id, offset) => (offset * 13 + id * 19) & 255;
const ram = new Cache({ budget: 65536, lanes: 4, background: 1, size: () => 1048583,
  fetch: async (id, start, end) => {
    order.push(id); calls++; bytes += end - start + 1; peak = Math.max(peak, ++active);
    await new Promise(resolve => setTimeout(resolve, 2)); active--;
    return Uint8Array.from({ length: end - start + 1 }, (_, i) => expected(id, start + i));
  }
});
// Two concurrent small reads share one 4 KB fetch, not two 256 KB fetches.
const [a, b] = await Promise.all([ram.read(0, 0, 15), ram.read(0, 10, 31)]);
assert.equal(calls, 1); assert.equal(bytes, 4096);
assert.deepEqual([...a], Array.from({ length: 16 }, (_, i) => expected(0, i)));
assert.deepEqual([...b], Array.from({ length: 22 }, (_, i) => expected(0, 10 + i)));
for (let id = 0; id < 12; id++) {
  const out = await ram.read(id, 333, 131400);
  assert.equal(out.length, 131068);
  for (let i = 0; i < out.length; i++) assert.equal(out[i], expected(id, 333 + i));
  assert(ram.bytes <= ram.budget);
}
const eof = await ram.read(2, 1048570, 1048582);
assert.equal(eof.length, 13); assert.equal(eof.at(-1), expected(2, 1048582));
const streamed = new Uint8Array(900000); let streamActive = 0, streamPeak = 0;
const parallel = new Cache({budget:65536,lanes:4,background:1,size:()=>1048576,fetch:async(id,a,z)=>{
  streamPeak=Math.max(streamPeak,++streamActive);
  await new Promise(resolve=>setTimeout(resolve,(1048576-a)/131072)); streamActive--;
  return Uint8Array.from({length:z-a+1},(_,i)=>expected(id,a+i));
}});
assert.equal(await parallel.transfer(4,333,streamed.length,(data,at)=>streamed.set(data,at)),streamed.length);
assert.equal(streamPeak,4);
for(let i=0;i<streamed.length;i++)assert.equal(streamed[i],expected(4,333+i));
assert(parallel.bytes<=parallel.budget);
await assert.rejects(ram.read(0, -1, 0));
await assert.rejects(ram.read(0, 0, 1048583));
// A demand read keeps a lane even while background work is queued.
const background = Array.from({ length: 12 }, (_, id) => ram.prefetch(30 + id, 0, 4095));
await ram.read(99, 0, 15); await Promise.all(background);
assert(peak <= 4);
assert(order.indexOf(99) < order.indexOf(31), 'demanda não deve ficar atrás da fila especulativa');
let attempts = 0;
const failing = new Cache({size: () => 4096, fetch: async () => { attempts++; if (attempts === 1) throw Error('offline'); return new Uint8Array(4096); }});
await assert.rejects(failing.read(0, 0, 31));
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal((await failing.read(0, 0, 31)).length, 32);
let manifestSize = 8192;
const shorter = new Cache({ size: () => manifestSize, fetch: async () => { manifestSize = 100; return new Uint8Array(100); }});
assert.equal((await shorter.read(0, 0, 4095)).length, 100);
manifestSize=1048576;
const shortParallel=new Cache({size:()=>manifestSize,fetch:async(id,a,z)=>{
  await new Promise(resolve=>setTimeout(resolve,1));manifestSize=100;
  return new Uint8Array(Math.max(0,Math.min(z+1,100)-a));
}});
assert.equal(await shortParallel.transfer(0,0,524288,()=>{}),100);
console.log(JSON.stringify({ passed: true, header_bytes: 4096, old_header_bytes: 262144, deduplicated: true, ram_limit: ram.budget, peak_requests: peak, retry: true }));
