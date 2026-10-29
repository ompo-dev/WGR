// WGR: bounded, demand-first RAM cache, with optional persistent range storage.
// Appended only to the verified legacy worker; the engine protocol stays intact.
class WgrPersistentCache {
  constructor(budget = 2147483648, name = 'wgr-block-cache-v1') {
    this.budget = budget; this.touched = new Map(); this.hits = 0; this.writes = 0;
    this.touchQueue = new Set(); this.touchTimer = 0; this.touchTransactions = 0; this.touchRecords = 0;
    this.onSlow = null; this.latency = {read_max_ms:0,write_max_ms:0,touch_max_ms:0};
    this.ready = new Promise(resolve => {
      if (!self.indexedDB) return resolve(null);
      const request = indexedDB.open(name, 1);
      request.onupgradeneeded = () => {
        const db = request.result;
        const entries = db.createObjectStore('entries', { keyPath: 'key' });
        entries.createIndex('access', 'access');
        entries.createIndex('offset', ['version', 'file', 'start']);
        db.createObjectStore('payload'); db.createObjectStore('meta');
      };
      request.onsuccess = () => { request.result.onversionchange = () => request.result.close(); resolve(request.result); };
      request.onerror = request.onblocked = () => resolve(null);
    });
  }
  async read(version, file, start, end) {
    const started = performance.now();
    const db = await this.ready; if (!db) return null;
    return new Promise(resolve => {
      const tx = db.transaction(['entries', 'payload'], 'readonly'), found = [];
      let position = start, result = null, complete = false, searched = 0;
      const seek = () => {
        const range = IDBKeyRange.bound([version, file, 0], [version, file, position]);
        const request = tx.objectStore('entries').index('offset').openCursor(range, 'prev');
        request.onsuccess = () => {
          const cursor = request.result; if (!cursor || searched++ >= 256) return;
          const entry = cursor.value;
          if (entry.end < position) return cursor.continue();
          const payload = tx.objectStore('payload').get(entry.key);
          payload.onsuccess = () => {
            if (!(payload.result instanceof ArrayBuffer) || payload.result.byteLength !== entry.bytes) return cursor.continue();
            result ??= new Uint8Array(end - start + 1);
            const last = Math.min(end, entry.end);
            result.set(new Uint8Array(payload.result, position - entry.start, last - position + 1), position - start);
            found.push(entry); position = last + 1;
            if (position > end) complete = true; else seek();
          };
        };
      };
      seek();
      tx.oncomplete = () => {
        this.measure('read',started,{file,offset:start,bytes:end-start+1,hit:complete});
        if (complete) { this.hits++; for (const entry of found) this.touch(entry); }
        resolve(complete ? result : null);
      };
      tx.onerror = tx.onabort = () => resolve(null);
    });
  }
  touch(entry) {
    const now = Date.now();
    if (now - (this.touched.get(entry.key) || 0) < 60000 || this.touchQueue.size >= 256) return;
    this.touched.delete(entry.key); this.touched.set(entry.key, now);
    if (this.touched.size > 8192) this.touched.delete(this.touched.keys().next().value);
    this.touchQueue.add(entry.key);
    if (!this.touchTimer) this.touchTimer = setTimeout(() => this.flushTouches(), 2000);
  }
  async flushTouches() {
    clearTimeout(this.touchTimer); this.touchTimer = 0;
    if(this.flushing)return this.flushing;
    const db = await this.ready; if (!db || !this.touchQueue.size) return;
    const keys = [...this.touchQueue]; this.touchQueue.clear();
    this.flushing=(async()=>{for(let at=0;at<keys.length;at+=8){
    const batch=keys.slice(at,at+8),started=performance.now();
    await new Promise(resolve => {
      try {
        const tx = db.transaction('entries', 'readwrite'), store = tx.objectStore('entries'), now = Date.now();
        for (const key of batch) {
          const request = store.get(key);
          request.onsuccess = () => { if (request.result) store.put({ ...request.result, access: now }); };
        }
        tx.oncomplete = () => { this.touchTransactions++; this.touchRecords += batch.length; this.measure('touch',started,{records:batch.length}); resolve(); };
        tx.onerror = tx.onabort = () => resolve();
      } catch { resolve(); }
    });
    // Release the store lock and let queued demand reads run between batches.
    if(at+8<keys.length)await new Promise(resolve=>setTimeout(resolve,0));
    }})().finally(()=>{this.flushing=null;});
    return this.flushing;
  }
  async put(version, file, start, bytes) {
    const started = performance.now();
    const db = await this.ready; if (!db || !bytes.length || bytes.length > this.budget) return;
    const data = bytes.buffer instanceof ArrayBuffer && !bytes.byteOffset && bytes.byteLength === bytes.buffer.byteLength ? bytes.buffer : bytes.slice().buffer;
    const end = start + bytes.length - 1, key = JSON.stringify([version, file, start, end]);
    await new Promise(resolve => {
      const tx = db.transaction(['entries', 'payload', 'meta'], 'readwrite');
      const entries = tx.objectStore('entries'), payload = tx.objectStore('payload'), meta = tx.objectStore('meta');
      const total = meta.get('bytes');
      total.onsuccess = () => {
        const previous = entries.get(key);
        previous.onsuccess = () => {
          let size = (total.result || 0) - (previous.result?.bytes || 0) + bytes.length;
          entries.put({ key, version, file, start, end, bytes: bytes.length, access: Date.now() }); payload.put(data, key);
          const evict = () => {
            if (size <= this.budget) { meta.put(size, 'bytes'); return; }
            const cursor = entries.index('access').openCursor();
            cursor.onsuccess = () => {
              const item = cursor.result;
              if (size > this.budget && item) { size -= item.value.bytes; payload.delete(item.value.key); item.delete(); item.continue(); }
              else meta.put(size, 'bytes');
            };
          };
          // A larger immutable range replaces contained fragments instead of
          // consuming quota for the same bytes twice.
          const covered = entries.index('offset').openCursor(IDBKeyRange.bound([version, file, start], [version, file, end]));
          covered.onsuccess = () => {
            const item = covered.result; if (!item) return evict();
            if (item.value.key !== key && item.value.end <= end) { size -= item.value.bytes; payload.delete(item.value.key); item.delete(); }
            item.continue();
          };
        };
      };
      tx.oncomplete = () => { this.writes++; this.measure('write',started,{file,offset:start,bytes:bytes.length}); resolve(); };
      // Storage pressure disables the cache write, never the engine's read.
      tx.onerror = tx.onabort = () => resolve();
    });
  }
  async usage() {
    const db = await this.ready; if (!db) return 0;
    return new Promise(resolve => { const request = db.transaction('meta').objectStore('meta').get('bytes'); request.onsuccess = () => resolve(request.result || 0); request.onerror = () => resolve(0); });
  }
  measure(kind,started,detail) {
    const duration_ms=performance.now()-started,key=kind+'_max_ms';
    this.latency[key]=Math.max(this.latency[key],duration_ms);
    if(duration_ms>=25)this.onSlow?.({kind,epoch_ms:Math.round(performance.timeOrigin+started),duration_ms,...detail});
  }
  async close() { await this.flushTouches(); (await this.ready)?.close(); }
}
self.WgrPersistentCache = WgrPersistentCache;
class WgrRamCache {
  constructor({ size, fetch, budget = 48 * 1048576, lanes = 12, background = 4 }) {
    Object.assign(this, { size, fetch, budget, lanes, background });
    this.blocks = new Map(); this.pending = new Map(); this.queue = [];
    this.bytes = 0; this.active = 0; this.lowActive = 0;
    this.stats = { fetched: 0, hits: 0, misses: 0 };
  }
  key(id, block) { return id + ':' + block; }
  touch(key) {
    const data = this.blocks.get(key);
    if (data) { this.blocks.delete(key); this.blocks.set(key, data); }
    return data;
  }
  pump() {
    while (this.active < this.lanes) {
      let at = this.queue.findIndex(job => !job.low);
      if (at < 0) {
        if (this.lowActive >= this.background) return;
        at = this.queue.findIndex(job => job.low);
      }
      if (at < 0) return;
      const job = this.queue.splice(at, 1)[0];
      this.active++; if (job.low) this.lowActive++;
      const wasLow = job.low;
      Promise.resolve().then(() => this.fetch(job.id, job.first * 4096, job.end, wasLow)).then(data => {
        const expected = job.end - job.first * 4096 + 1;
        if (!(data instanceof Uint8Array) || data.length > expected || (data.length < expected && this.size(job.id) !== job.first * 4096 + data.length)) throw Error('Leitura RAM incompleta');
        this.stats.fetched += data.length;
        for (let b = job.first; b <= job.last; b++) {
          const key = this.key(job.id, b), offset = (b - job.first) * 4096;
          const block = data.slice(offset, Math.min(data.length, offset + 4096));
          const old = this.blocks.get(key); if (old) this.bytes -= old.length;
          this.blocks.delete(key); this.blocks.set(key, block); this.bytes += block.length;
        }
        while (this.bytes > this.budget) {
          const key = this.blocks.keys().next().value;
          this.bytes -= this.blocks.get(key).length; this.blocks.delete(key);
        }
        job.resolve(data);
      }).catch(job.reject).finally(() => {
        for (let b = job.first; b <= job.last; b++) this.pending.delete(this.key(job.id, b));
        this.active--; if (wasLow) this.lowActive--; this.pump();
      });
    }
  }
  request(id, first, last, low) {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    const job = { id, first, last, low, end: Math.min(this.size(id), (last + 1) * 4096) - 1, promise, resolve, reject };
    for (let b = first; b <= last; b++) this.pending.set(this.key(id, b), job);
    this.queue.push(job); this.pump(); return job;
  }
  async read(id, start, end, low = false) {
    const size = this.size(id);
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < 0 || end < start || end >= size || end - start >= 131072) throw Error('Intervalo RAM inválido');
    const first = Math.floor(start / 4096), last = Math.floor(end / 4096), jobs = new Set(), cached = new Map();
    for (let b = first; b <= last;) {
      const key = this.key(id, b), hit = this.touch(key);
      if (hit) { cached.set(b, hit); this.stats.hits++; b++; continue; }
      this.stats.misses++;
      let job = this.pending.get(key);
      if (job) { if (!low) job.low = false; jobs.add(job); b++; continue; }
      const a = b;
      while (b < last && !this.blocks.has(this.key(id, b + 1)) && !this.pending.has(this.key(id, b + 1))) b++;
      job = this.request(id, a, b++, low); jobs.add(job);
    }
    this.pump();
    const replies = await Promise.all([...jobs].map(async job => [job, await job.promise]));
    end = Math.min(end, this.size(id) - 1);
    const result = new Uint8Array(Math.max(0, end - start + 1));
    for (let b = first; b <= Math.floor(end / 4096); b++) {
      let data = cached.get(b);
      if (!data) {
        const [job, bytes] = replies.find(([job]) => b >= job.first && b <= job.last);
        data = bytes.subarray((b - job.first) * 4096, (b - job.first + 1) * 4096);
      }
      const a = Math.max(start, b * 4096), z = Math.min(end + 1, b * 4096 + data.length);
      result.set(data.subarray(a - b * 4096, z - b * 4096), a - start);
    }
    return result;
  }
  prefetch(id, start, end) {
    if (this.queue.length >= 48 || start >= this.size(id)) return Promise.resolve();
    return this.read(id, start, Math.min(end, this.size(id) - 1, start + 131071), true).catch(() => {});
  }
  async transfer(id, off, len, write) {
    let done = 0;
    while (done < len && off + done < this.size(id)) {
      const jobs = [];
      for (let at = done; at < len && jobs.length < 4; at += 131072) {
        const start = off + at, n = Math.min(131072, len - at, this.size(id) - start);
        if (n <= 0) break;
        jobs.push({ n, promise: this.read(id, start, start + n - 1).catch(error => {
          if (start >= this.size(id)) return new Uint8Array();
          throw error;
        }) });
      }
      const replies = await Promise.all(jobs.map(job => job.promise));
      for (let i = 0; i < replies.length; i++) {
        write(replies[i], done); done += replies[i].length;
        if (replies[i].length < jobs[i].n) return done;
      }
    }
    return done;
  }
}
self.WgrRamCache = WgrRamCache;

// Streaming audio issues short sequential reads. Preserve the original store's
// read-ahead even in the bounded noStore path; never wait for speculative bytes.
function wgrAudioAhead(previous,off,len,size) {
  const sequential=previous&&off>=previous.end&&off-previous.end<=65536;
  const ahead=sequential?Math.min(1048576,previous.ahead?previous.ahead*2:131072):0;
  const end=off+len,ranges=[];
  for(let start=Math.ceil(end/4096)*4096;start<Math.min(size,end+ahead);start+=131072)
    ranges.push([start,Math.min(size,end+ahead,start+131072)-1]);
  return {end,ahead,ranges};
}
self.wgrAudioAhead=wgrAudioAhead;

// Headers first; full boot ranges only when a persistent tier can retain them.
// Lazy, block-aligned, merged jobs avoid an unbounded Promise/request fan-out.
function* wgrBootRanges(set, index, size, full) {
  if (!Array.isArray(set)) return;
  for (const entry of set) {
    if (!Array.isArray(entry)) continue;
    const id = index.get(entry[0]), length = id === undefined ? 0 : size(id);
    if (length > 0) yield [id, 0, Math.min(4095, length - 1)];
  }
  if (!full) return;
  for (const entry of set) {
    if (!Array.isArray(entry) || !Array.isArray(entry[1])) continue;
    const id = index.get(entry[0]), length = id === undefined ? 0 : size(id);
    if (!(length > 4096)) continue;
    const ranges = entry[1].filter(r => Array.isArray(r) && Number.isSafeInteger(r[0]) && Number.isSafeInteger(r[1]) && r[0] >= 0 && r[1] >= r[0] && r[0] < length)
      .map(([a, z]) => [Math.max(4096, Math.floor(a / 4096) * 4096), Math.min(length - 1, Math.ceil((z + 1) / 4096) * 4096 - 1)])
      .sort((a, b) => a[0] - b[0]);
    let first = -1, end = -1;
    function* chunks(a, z) { for (; a <= z; a += 131072) yield [id, a, Math.min(z, a + 131071)]; }
    for (const [a, z] of ranges) {
      if (z < a) continue;
      if (first >= 0 && a > end + 1) { yield* chunks(first, end); first = -1; }
      if (first < 0) first = a;
      end = Math.max(end, z);
    }
    if (first >= 0) yield* chunks(first, end);
  }
}
self.wgrBootRanges = wgrBootRanges;

if (typeof readWithoutStore === 'function' && typeof init === 'function') {
  const persistent = self.WGR_PERSISTENT_CACHE ? new WgrPersistentCache() : null;
  let networkBytes = 0, cacheTimer = 0, lastCacheStats = '';
  let ioWindow={reads:0,bytes:0,total_ms:0,max_ms:0},ioWindowT=performance.now();
  const audioSequences=new Map();let audioAheadBytes=0;
  let reportSecond=0,reportCounts={},ioEventsDropped=0;
  const reportSlow=(key,event)=>{
    const second=Math.floor(Date.now()/1000);if(second!==reportSecond){reportSecond=second;reportCounts={};}
    const kind=key+':'+event.kind;if((reportCounts[kind]||0)>=20){ioEventsDropped++;return;}
    reportCounts[kind]=(reportCounts[kind]||0)+1;bc.postMessage({[key]:event});
  };
  const measureIo=(kind,started,detail)=>{const duration_ms=performance.now()-started;if(duration_ms>=25)reportSlow('io_stall',{kind,epoch_ms:Math.round(performance.timeOrigin+started),duration_ms,...detail});};
  if(persistent)persistent.onSlow=event=>reportSlow('cache_stall',event);
  const ram = new WgrRamCache({
    lanes: persistent ? 32 : 12, background: persistent ? 8 : 4,
    size: id => files[id].size,
    fetch: async (id, start, end, low) => {
      if (persistent) {
        const cached = await persistent.read(dataVersion, files[id].path, start, end);
        if (cached) return cached;
      }
      const bytes = new Uint8Array(end - start + 1);
      const started=performance.now();
      const got = await fetchRange(files[id], start, end, (chunk, at) => bytes.set(chunk, at), false, low);
      measureIo('network',started,{file:files[id].path,offset:start,bytes:got,background:low});
      networkBytes += got;
      if (got < bytes.length) { shrink(files[id], start + got); return bytes.subarray(0, got); }
      if (persistent) await persistent.put(dataVersion, files[id].path, start, bytes);
      return bytes;
    }
  });
  readWithoutStore = async (id, off, len, dst) => {
    const started = performance.now(), before = ram.stats.misses;
    const done = await ram.transfer(id, off, len, (data, at) => { views(); HEAPU8.set(data, dst + at); });
    if(/\/audio\/sfx\/(RADIO_|INTERACTIVE_MUSIC)/i.test('/'+files[id].path)) {
      const state=wgrAudioAhead(audioSequences.get(id),off,done,files[id].size);
      audioSequences.set(id,{end:state.end,ahead:state.ahead});
      if(audioSequences.size>128)audioSequences.delete(audioSequences.keys().next().value);
      for(const [start,end] of state.ranges){audioAheadBytes+=end-start+1;void ram.prefetch(id,start,end);}
    }
    const duration=performance.now()-started;ioWindow.reads++;ioWindow.bytes+=done;ioWindow.total_ms+=duration;ioWindow.max_ms=Math.max(ioWindow.max_ms,duration);
    measureIo('demand',started,{file:files[id].path,offset:off,bytes:len,misses:ram.stats.misses-before});
    if (ram.stats.misses > before) {
      netStat.demandMiss++; netStat.demandWaitMs += performance.now() - started;
    }
    return done;
  };
  const originalDrain = drainHints;
  drainHints = bell => {
    if (sh) return originalDrain(bell);
    if (noHints) return;
    const ring = Atomics.load(HEAP32, bell + 2), cap = Atomics.load(HEAP32, bell + 3);
    if (!ring || !cap) return;
    const head = Atomics.load(HEAP32, bell + 4) >>> 0;
    let tail = Atomics.load(HEAP32, bell + 5) >>> 0;
    if (((head - tail) >>> 0) > cap) tail = (head - cap) >>> 0;
    while (tail !== head && ram.queue.length < 48) {
      const e = bell + ring + 4 * (tail % cap), raw = Atomics.load(HEAP32, e + 3);
      if (!raw) break;
      const id = HEAP32[e], off = (HEAP32[e + 1] >>> 0) + (HEAP32[e + 2] >>> 0) * 4294967296;
      const len = raw & 0x3fffffff;
      if (files[id] && len > 0) ram.prefetch(id, off, off + len - 1);
      Atomics.store(HEAP32, e + 3, 0); tail = (tail + 1) >>> 0;
    }
    Atomics.store(HEAP32, bell + 5, tail);
  };
  const originalInit = init;
  init = async message => {
    if (persistent) message = { ...message, noStore: true };
    await originalInit(message);
    if (persistent && !cacheTimer) cacheTimer = setInterval(async () => {
      const now=performance.now();bc.postMessage({io_window:{epoch_ms:Math.round(performance.timeOrigin+now),span_ms:now-ioWindowT,...ioWindow}});ioWindow={reads:0,bytes:0,total_ms:0,max_ms:0};ioWindowT=now;
      const cache = { disk_hits: persistent.hits, ram_hits: ram.stats.hits, network_bytes: networkBytes, disk_bytes: await persistent.usage(), disk_limit: persistent.budget, disk_writes: persistent.writes, touch_transactions: persistent.touchTransactions, touch_records: persistent.touchRecords,latency:persistent.latency,io_events_dropped:ioEventsDropped,audio_ahead_requested_bytes:audioAheadBytes };
      const current = JSON.stringify(cache);
      if (current !== lastCacheStats) { lastCacheStats = current; bc.postMessage({ cache }); }
    }, 5000);
    if (!message.noStore || !message.bootset || message.noHints) return;
    (async () => {
      const response = await fetchRetry(base + message.bootset, { cache: 'no-store' });
      if (!response.ok) return;
      const jobs = wgrBootRanges(await response.json(), pathIndex, id => files[id].size, !!persistent);
      await Promise.all(Array.from({ length: persistent ? 8 : 4 }, async () => {
        for (let job = jobs.next(); !job.done; job = jobs.next()) {
          await ram.prefetch(...job.value);
        }
      }));
    })().catch(error => log('[WGR RAM] ' + error.message));
    log('[WGR cache] 4 KB blocks; 48 MiB RAM; ' + (persistent ? 'persistent ranges limited to 2 GiB' : 'RAM only'));
  };
}
