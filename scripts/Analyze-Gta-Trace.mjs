import {readFile,writeFile,readdir} from 'node:fs/promises';
import path from 'node:path';
const [reportPath,resourcePath]=process.argv.slice(2);
if(!reportPath||!resourcePath)throw Error('Uso: node scripts/Analyze-Gta-Trace.mjs report.json monitor.json');
const report=JSON.parse(await readFile(reportPath,'utf8'));
const resourceData=JSON.parse(await readFile(resourcePath,'utf8'));
const resources=Array.isArray(resourceData)?resourceData:[resourceData];
const eventPath=resourcePath.replace(/\.json$/,'-events.ndjson');
let rows=report.timeline||[];
try{rows=(await readFile(eventPath,'utf8')).trim().split('\n').filter(Boolean).map(line=>JSON.parse(line));}catch{}
const events=rows.flatMap(row=>Object.entries(row.payload).map(([type,value])=>({type,...value})));
const frames=events.filter(e=>e.type==='gpu_frame');
const percentile=(values,p)=>{const a=values.filter(Number.isFinite).sort((a,b)=>a-b);return a.length?a[Math.floor((a.length-1)*p)]:null;};
const overlap=(frame,event)=>Math.max(0,Math.min(frame.epoch_ms,event.epoch_ms+event.duration_ms)-Math.max(frame.epoch_ms-frame.duration_ms,event.epoch_ms));
const sum=(items,key)=>items.reduce((n,item)=>n+(Number(item[key])||0),0);
const latest=events.findLast(e=>e.type==='gpu_memory');
const windows=events.filter(e=>e.type==='gpu_window'&&e.world);
const cpu=[];
const prefix=path.basename(reportPath,'.json')+'.trace.worker-';
for(const filename of await readdir(path.dirname(reportPath))){
  if(!filename.startsWith(prefix)||!filename.endsWith('.cpuprofile'))continue;
  const raw=JSON.parse(await readFile(path.join(path.dirname(reportPath),filename),'utf8'));
  const profile=raw.result?.profile||raw.profile;
  if(!profile?.nodes)continue;
  const nodes=new Map(profile.nodes.map(node=>[node.id,node]));
  const weights=new Map();let total=0;
  for(let i=0;i<(profile.samples||[]).length;i++){
    const id=profile.samples[i],weight=profile.timeDeltas?.[i]||5000;
    weights.set(id,(weights.get(id)||0)+weight);total+=weight;
  }
  cpu.push({file:filename,approx_start_epoch_ms:raw.approx_start_epoch_ms,samples:profile.samples?.length,span_ms:(profile.endTime-profile.startTime)/1000,top_self:[...weights].sort((a,b)=>b[1]-a[1]).slice(0,15).map(([id,weight])=>({name:nodes.get(id)?.callFrame?.functionName,url:nodes.get(id)?.callFrame?.url,weighted_sample_percent:Math.round(weight*10000/Math.max(1,total))/100}))});
}
let traceSummary=null;
try{
  const trace=JSON.parse(await readFile(reportPath.replace(/\.json$/,'.trace.json'),'utf8'));
  const events=trace.traceEvents||[];
  const gc=events.filter(e=>e.ph==='X'&&/GC/.test(e.name));
  const byProcess=Object.groupBy(events.filter(e=>e.ph!=='M'&&e.ts),e=>e.pid);
  traceSummary={events:events.length,max_gc_ms:Math.max(0,...gc.map(e=>e.dur/1000)),process_spans_ms:Object.entries(byProcess).map(([pid,items])=>({pid:Number(pid),span_ms:(Math.max(...items.map(e=>e.ts))-Math.min(...items.map(e=>e.ts)))/1000})),warning:'A duração por processo pode diferir por descarte no buffer. Ausência de evento não prova ausência de pausa.'};
}catch{}
const summary={
  world_at_ms:report.world_at_ms,cpu_profiles:report.cpu_profiles,trace:traceSummary,
  system_available_min_mib:Math.min(...resources.map(r=>r.system_memory?.AvailableMBytes??Infinity)),
  peak_working_gib:Math.max(...resources.map(r=>sum(r.members||[],'working_bytes')/1073741824)),
  peak_gpu_dedicated_gib:Math.max(...resources.map(r=>sum(r.gpu||[],'dedicated_bytes')/1073741824)),
  latest_gpu_memory:latest,cache:report.cache,
  recorded_slow_frames:frames.length,slow_frame_median_idle_percent:percentile(frames.map(f=>100*f.idle_ms/f.duration_ms),.5),
  window_median_p50_ms:percentile(windows.map(w=>w.p50_ms),.5),window_median_p99_ms:percentile(windows.map(w=>w.p99_ms),.5),
  event_counts:Object.fromEntries(Object.entries(Object.groupBy(events,e=>e.type)).map(([key,values])=>[key,values.length])),
  worst_frames:[...frames].sort((a,b)=>b.duration_ms-a.duration_ms).slice(0,20).map(frame=>({
    ...frame,relations:['cache_stall','io_stall','gpu_async'].map(type=>{const related=events.filter(e=>e.type===type&&overlap(frame,e)>5).sort((a,b)=>overlap(frame,b)-overlap(frame,a));return{type,count:related.length,best:related.slice(0,3).map(e=>({kind:e.kind,file:e.file,duration_ms:e.duration_ms,overlap_ms:overlap(frame,e)}))};})
  })),workers:cpu,
  limits:'FPS de submissão do port, não apresentação física. Working sets podem conter páginas compartilhadas. I/O por processo inclui IPC/rede. Paginação é global e amostrada. CPU usa amostras de parede, inclui espera e tem alinhamento aproximado. Sobreposição temporal não prova causalidade.'
};
const output=reportPath.replace(/\.json$/,'.analysis.json');
await writeFile(output,JSON.stringify(summary,null,2)+'\n');
console.log(JSON.stringify({output,...Object.fromEntries(Object.entries(summary).filter(([k])=>!['worst_frames','workers','latest_gpu_memory','cache'].includes(k)))},null,2));
