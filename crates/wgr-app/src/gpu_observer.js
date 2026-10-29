// Local, bounded diagnostics; appended only to the hash-verified GPU worker.
const wgrFrameTimes = new Float32Array(1024);
let wgrFrameIndex = 0, wgrFrameCount = 0, wgrMemoryReportT = 0, wgrWorld = false;
function wgrGpuWait(kind, started) {
  const duration_ms=performance.now()-started;
  if(!worldSent||duration_ms<25)return;
  progressChannel ??= new BroadcastChannel('game-progress');
  progressChannel.postMessage({gpu_async:{kind,epoch_ms:Math.round(performance.timeOrigin+started),duration_ms}});
}
function wgrRecordFrame(dt, tn) {
  if (worldSent !== wgrWorld) { wgrFrameIndex = wgrFrameCount = 0; wgrWorld = worldSent; }
  wgrFrameTimes[wgrFrameIndex++ % 1024] = dt;
  wgrFrameCount = Math.min(1024, wgrFrameCount + 1);
  if (!worldSent || dt <= 40) return;
  progressChannel ??= new BroadcastChannel('game-progress');
  progressChannel.postMessage({gpu_frame:{epoch_ms:Math.round(performance.timeOrigin + tn),duration_ms:dt,idle_ms:fr.idle,pipeline_ms:fr.pipe,module_ms:fr.mod,upload_ms:fr.up,upload_bytes:fr.upBytes,gpu_wait_ms:fr.wait,submit_ms:fr.flush,compiling:pipelinesPending}});
}
function wgrReportGpu(tn) {
  if (tn - wgrMemoryReportT < 5000) return;
  const times = Array.from(wgrFrameTimes.subarray(0,wgrFrameCount)).sort((a,b)=>a-b);
  const percentile = p => times.length ? times[Math.floor((times.length-1)*p)] : 0;
  progressChannel ??= new BroadcastChannel('game-progress');
  progressChannel.postMessage({gpu_memory:{epoch_ms:Math.round(performance.timeOrigin + tn),texture_bytes:texBytesAlive,buffer_bytes:bufBytesAlive,uniform_ring_bytes:ubo.shared.length*FRAME_SLOTS*RING_CHUNK,pooled_buffer_bytes:[...bufferPool].reduce((n,[k,v])=>n+(k%4294967296)*v.length,0),texture_count:texCount,pipeline_entries:contentPipelines.size,shader_module_creates:shaderModuleCount,bind_group_cache:bindGroupCache.size,compiled_ahead:aheadMade,used_ahead:aheadUsed},gpu_window:{epoch_ms:Math.round(performance.timeOrigin + tn),world:worldSent,frames:times.length,span_ms:tn-wgrMemoryReportT,p50_ms:percentile(.5),p95_ms:percentile(.95),p99_ms:percentile(.99),max_ms:percentile(1)}});
  wgrFrameIndex = wgrFrameCount = 0; wgrMemoryReportT = tn;
}
