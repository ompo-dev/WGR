// Independent, deterministic comparison with tests/encoder_reference.rs.
// Uses a disposable Chromium profile and localhost only, no GTA assets.
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
const output=resolve(process.argv[2] || 'artifacts/dawn-encoder-reference');
const browserPath=process.argv[3] || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
await mkdir(output,{recursive:true});
const profile=join(output,'isolated-'+Date.now());
const shader=await readFile(new URL('./fixtures/encoder-reference.wgsl',import.meta.url),'utf8');
const page=`<!doctype html><meta charset=utf-8><title>WGR Dawn fixture</title><pre id=out>Running</pre><script type=module>
try {
 const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});
 if(!adapter)throw Error('WebGPU adapter unavailable');
 const device=await adapter.requestDevice();
 device.pushErrorScope('validation');
 const stride=Math.max(256,device.limits.minUniformBufferOffsetAlignment);
 const constants=device.createBuffer({size:stride*256,usage:GPUBufferUsage.UNIFORM|GPUBufferUsage.COPY_DST});
 const data=new Float32Array(stride*256/4);
 for(let i=0;i<256;i++)data.set(i%2?[0,1,0,1]:[1,0,0,1],i*stride/4);
 device.queue.writeBuffer(constants,0,data);
 const layout=device.createBindGroupLayout({entries:[{binding:0,visibility:GPUShaderStage.FRAGMENT,buffer:{type:'uniform',hasDynamicOffset:true,minBindingSize:16}}]});
 const bg=device.createBindGroup({layout,entries:[{binding:0,resource:{buffer:constants,size:16}}]});
 const module=device.createShaderModule({code:${JSON.stringify(shader)}});
 const pipeline=device.createRenderPipeline({layout:device.createPipelineLayout({bindGroupLayouts:[layout]}),vertex:{module,entryPoint:'vs'},fragment:{module,entryPoint:'fs',targets:[{format:'rgba8unorm'}]}});
 const target=device.createTexture({size:[32,32],format:'rgba8unorm',usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.COPY_SRC});
 const view=target.createView();
 const read=device.createBuffer({size:256,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ});
 const report={adapter:{vendor:adapter.info.vendor,architecture:adapter.info.architecture,device:adapter.info.device,description:adapter.info.description},samples:[]};
 for(const [passes,draws] of [[1,3200],[320,10],[800,4]]){
  const samples=[];
  for(let run=0;run<13;run++){
   const started=performance.now(),encoder=device.createCommandEncoder();
   for(let p=0;p<passes;p++){
    const pass=encoder.beginRenderPass({colorAttachments:[{view,loadOp:'clear',storeOp:'store',clearValue:[0,0,0,1]}]});
    pass.setPipeline(pipeline);
    for(let d=0;d<draws;d++){pass.setBindGroup(0,bg,[((p*draws+d)%256)*stride]);pass.draw(3);}
    pass.end();
   }
   encoder.copyTextureToBuffer({texture:target},{buffer:read,bytesPerRow:256,rowsPerImage:1},{width:1,height:1,depthOrArrayLayers:1});
   const encoded=performance.now(),commands=encoder.finish(),finished=performance.now();
   device.queue.submit([commands]);const queued=performance.now();
   await read.mapAsync(GPUMapMode.READ);
   const pixel=Array.from(new Uint8Array(read.getMappedRange(),0,4));read.unmap();
   const ended=performance.now();
   if(pixel.join(',')!=='0,255,0,255')throw Error('Invalid pixel '+pixel);
   if(run>=3)samples.push({record_ms:encoded-started,finish_ms:finished-encoded,submit_ms:queued-finished,total_ms:ended-started});
  }
  report.samples.push({passes,draws_per_pass:draws,samples,pixel:[0,255,0,255]});
 }
 const error=await device.popErrorScope();if(error)throw Error(error.message);
 document.getElementById('out').textContent=JSON.stringify(report,null,2);
 await fetch('/result',{method:'POST',body:JSON.stringify(report)});
} catch(e){await fetch('/result',{method:'POST',body:JSON.stringify({error:String(e.stack||e)})});}
</script>`;
let finish;
const completed=new Promise(r=>{finish=r});
const server=createServer((req,res)=>{
 if(req.method==='POST'&&req.url==='/result'){
  let body='';req.on('data',x=>{body+=x;if(body.length>1024*1024)req.destroy()});
  req.on('end',()=>{res.end('ok');try{finish(JSON.parse(body))}catch(e){finish({error:String(e)})}});
 }else{res.setHeader('Content-Type','text/html; charset=utf-8');res.end(page)}
});
await new Promise((ok,fail)=>server.once('error',fail).listen(0,'127.0.0.1',ok));
const child=spawn(browserPath,[`--user-data-dir=${profile}`,'--no-first-run','--no-default-browser-check','--remote-debugging-port=0',`http://127.0.0.1:${server.address().port}/`],{stdio:'ignore',windowsHide:false});
try{
 const result=await Promise.race([completed,sleep(90000,undefined,{ref:false}).then(()=>({error:'Fixture timeout'})),new Promise(r=>child.once('error',e=>r({error:String(e)})))]);
 // Query the exact isolated browser for backend information, not other profiles.
 try{
  const [port,path]=String(await readFile(join(profile,'DevToolsActivePort'),'utf8')).trim().split(/\r?\n/);
  const ws=new WebSocket('ws://127.0.0.1:'+port+path);
  await new Promise((ok,fail)=>{ws.addEventListener('open',ok,{once:true});ws.addEventListener('error',fail,{once:true});});
  const info=await Promise.race([new Promise(ok=>{ws.addEventListener('message',e=>{const m=JSON.parse(e.data);if(m.id===1)ok(m.result)});ws.send(JSON.stringify({id:1,method:'SystemInfo.getInfo'}));}),sleep(5000,undefined,{ref:false}).then(()=>null)]);
  result.browser_gpu={devices:info?.gpu?.devices,featureStatus:info?.gpu?.featureStatus,auxAttributes:info?.gpu?.auxAttributes};
  ws.send(JSON.stringify({id:2,method:'Browser.close'}));await sleep(500);ws.close();
 }catch(e){result.browser_info_error=String(e)}
 await writeFile(join(output,'dawn.json'),JSON.stringify(result,null,2));
 console.log(JSON.stringify(result));if(result.error)process.exitCode=1;
}finally{server.close();child.kill()}
