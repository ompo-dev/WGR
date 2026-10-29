'use strict';
(async()=>{
 const canvas=document.querySelector('#game'),ctx=canvas.getContext('2d');const keys=new Set();let x=480,y=210,score=0,frames=0,db=null,audio=null;
 const report=(type,data={})=>window.chrome?.webview?.postMessage(JSON.stringify({type,...data}));
 await new Promise((resolve,reject)=>{const script=document.createElement('script');script.src='/__wgr_ram_io.js';script.onload=resolve;script.onerror=reject;document.head.appendChild(script);});
 const downloads=new self.WgrPersistentCache(16384,'orbit-download-reopen');
 const cached=await downloads.read('v1','file',0,15),cacheRestored=!!cached;
 if(!cached)await downloads.put('v1','file',0,new Uint8Array(4096).fill(19));
 await downloads.close();
 const reopened=new self.WgrPersistentCache(16384,'orbit-download-reopen');
 if((await reopened.read('v1','file',0,15))?.[0]!==19)throw Error('Cache persistente não reabriu');
 if(await reopened.read('v2','file',0,15))throw Error('Cache misturou versões');
 await reopened.close();
 const quota=new self.WgrPersistentCache(8192,'orbit-download-quota');
 for(const file of ['a','b','c'])await quota.put('v1',file,0,new Uint8Array(4096).fill(7));
 if(await quota.usage()>8192||await quota.read('v1','a',0,15))throw Error('Quota/LRU não respeitada');
 await quota.close();
 await new Promise((resolve,reject)=>{const r=indexedDB.deleteDatabase('orbit-download-fragments');r.onsuccess=resolve;r.onerror=reject;});
 const fragments=new self.WgrPersistentCache(16384,'orbit-download-fragments');
 await fragments.put('v1','span',0,new Uint8Array(4096).fill(19));
 await fragments.put('v1','span',4096,new Uint8Array(4096).fill(23));
 const joined=await fragments.read('v1','span',4064,4127);
 if(joined?.length!==64||joined[0]!==19||joined[32]!==23)throw Error('Intervalos persistentes adjacentes não foram reunidos');
 if(await fragments.read('v1','span',8190,8194))throw Error('Cache aceitou intervalo com lacuna');
 await fragments.flushTouches();
 if(fragments.touchTransactions!==1||fragments.touchRecords!==2)throw Error('Metadados do cache não foram gravados em lote');
 const merged=new Uint8Array(8192).fill(19);merged.fill(23,4096);
 await fragments.put('v1','span',0,merged);
 if(await fragments.usage()!==8192)throw Error('Cache duplicou bytes de intervalos contidos');
 await fragments.close();
 const split=new self.WgrPersistentCache(16384,'orbit-download-small-batches');
 for(let i=0;i<20;i++){await split.put('v1','f'+i,0,new Uint8Array(256));await split.read('v1','f'+i,0,15);}
 await split.flushTouches();
 if(split.touchTransactions!==3||split.touchRecords!==20)throw Error('Metadados bloquearam o cache com lote maior que oito');
 await split.close();
 const {instance}=await WebAssembly.instantiateStreaming(fetch('math.wasm'));const step=instance.exports.step;
 const worker=new Worker('worker.js');const workerReady=new Promise((resolve,reject)=>{worker.onmessage=e=>resolve(e.data);worker.onerror=reject;});worker.postMessage({type:'probe'});
 const workerResult=await workerReady;
 const range=await fetch('data/probe.bin',{headers:{Range:'bytes=2-5'}});const rangeBytes=await range.text();
 if(range.status!==206||rangeBytes!=='2345')throw Error('Range não corresponde aos bytes esperados');
 const batch=await fetch('data/batch',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify([['probe.bin',0,1],['probe.bin',8,9]])});
 if(await batch.text()!=='0189'||batch.headers.get('x-run-lengths')!=='2,2')throw Error('Batch não corresponde');
 let restored=false;try{db=await new Promise((resolve,reject)=>{const r=indexedDB.open('orbit-save',1);r.onupgradeneeded=()=>r.result.createObjectStore('save');r.onsuccess=()=>resolve(r.result);r.onerror=reject;});const saved=await new Promise(resolve=>{const r=db.transaction('save').objectStore('save').get('progress');r.onsuccess=()=>resolve(r.result);r.onerror=()=>resolve(null);});if(saved){x=saved.x;y=saved.y;score=saved.score;restored=true;}document.querySelector('#save').textContent='Save local: pronto';}catch(e){document.querySelector('#save').textContent='Save indisponível: '+e.message;}
 function save(){if(db){const tx=db.transaction('save','readwrite');tx.objectStore('save').put({x,y,score},'progress');}}
 let orb={x:690,y:210};
 function input(dx,dy){x=Math.max(14,Math.min(946,step(Math.round(x),dx)));y=Math.max(14,Math.min(406,step(Math.round(y),dy)));report('fixture_input',{x,y});}
 addEventListener('keydown',e=>{if(['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','w','a','s','d'].includes(e.key)){e.preventDefault();keys.add(e.key);}});addEventListener('keyup',e=>keys.delete(e.key));addEventListener('blur',()=>keys.clear());
 canvas.addEventListener('pointerdown',e=>{canvas.focus();const r=canvas.getBoundingClientRect();x=Math.round((e.clientX-r.left)*960/r.width);y=Math.round((e.clientY-r.top)*420/r.height);report('fixture_input',{x,y});save();});
 document.querySelector('#audio').onclick=async()=>{try{audio??=new AudioContext();await audio.audioWorklet.addModule('tone.js');const tone=new AudioWorkletNode(audio,'orbit-tone');tone.connect(audio.destination);await audio.resume();setTimeout(()=>tone.disconnect(),250);document.querySelector('#audio').textContent='Áudio ativado';report('fixture_audio',{state:audio.state});}catch(e){document.querySelector('#audio').textContent='Áudio: '+e.message;}};
 document.querySelector('#gpu').onclick=async()=>{try{const a=await navigator.gpu?.requestAdapter();if(!a)throw Error('Adaptador indisponível');const d=await a.requestDevice();const c=new OffscreenCanvas(8,8).getContext('webgpu');c.configure({device:d,format:navigator.gpu.getPreferredCanvasFormat()});const encoder=d.createCommandEncoder();const pass=encoder.beginRenderPass({colorAttachments:[{view:c.getCurrentTexture().createView(),loadOp:'clear',storeOp:'store',clearValue:{r:0.2,g:0.9,b:0.7,a:1}}]});pass.end();d.queue.submit([encoder.finish()]);await d.queue.onSubmittedWorkDone();d.destroy();document.querySelector('#gpu').textContent='WebGPU: quadro enviado';report('fixture_webgpu',{rendered:true});}catch(e){document.querySelector('#gpu').textContent='WebGPU: '+e.message;report('fixture_webgpu',{rendered:false,error:e.message});}};
 let last=performance.now(),fpsFrames=0,lastSave=0;
 function frame(t){const dx=(keys.has('ArrowRight')||keys.has('d')?4:0)-(keys.has('ArrowLeft')||keys.has('a')?4:0),dy=(keys.has('ArrowDown')||keys.has('s')?4:0)-(keys.has('ArrowUp')||keys.has('w')?4:0);if(dx||dy)input(dx,dy);
 const pad=navigator.getGamepads?.()[0];if(pad&&(Math.abs(pad.axes[0])>.2||Math.abs(pad.axes[1])>.2))input(Math.round(pad.axes[0]*5),Math.round(pad.axes[1]*5));
 if(Math.hypot(x-orb.x,y-orb.y)<28){score++;orb={x:50+Math.random()*860,y:50+Math.random()*320};save();}
 ctx.fillStyle='#161d28';ctx.fillRect(0,0,960,420);ctx.strokeStyle='#202b3c';ctx.lineWidth=1;for(let i=0;i<960;i+=40){ctx.beginPath();ctx.moveTo(i,0);ctx.lineTo(i,420);ctx.stroke();}for(let i=0;i<420;i+=40){ctx.beginPath();ctx.moveTo(0,i);ctx.lineTo(960,i);ctx.stroke();}
 for(let i=0;i<18;i++){ctx.fillStyle='#46596f';ctx.fillRect((i*137)%940,(i*67)%400,2,2);}ctx.strokeStyle='#3a6b60';ctx.beginPath();ctx.arc(orb.x,orb.y,22+Math.sin(t/500)*4,0,Math.PI*2);ctx.stroke();ctx.fillStyle='#8bebc6';ctx.beginPath();ctx.arc(orb.x,orb.y,6,0,Math.PI*2);ctx.fill();
 ctx.shadowColor='#9bbdf5';ctx.shadowBlur=20;ctx.fillStyle='#a9c8ff';ctx.beginPath();ctx.arc(x,y,11,0,Math.PI*2);ctx.fill();ctx.shadowBlur=0;document.querySelector('#score').textContent=score+' luzes';frames++;fpsFrames++;
 if(frames===2)report('first_frame',{engine:'orbit-wasm-canvas',frames,worker:workerResult,range:true,batch:true,save:!!db,restored,download_cache:{restored:cacheRestored,reopened:true,quota:true,fragments:true},position:{x,y,score}});if(t-last>=1000){report('fixture_fps',{fps:fpsFrames*1000/(t-last),frames});last=t;fpsFrames=0;}if(t-lastSave>2000){save();lastSave=t;}requestAnimationFrame(frame);}
 document.querySelector('#mode').textContent='WASM + Canvas 2D · offline';document.querySelector('#checks').textContent=`WebAssembly ✓   Worker ✓   OffscreenCanvas ${workerResult.offscreen?'✓':'indisponível'}   Range ✓   Batch ✓   Isolamento ${crossOriginIsolated?'✓':'indisponível'}`;
 addEventListener('pagehide',()=>{save();worker.terminate();audio?.close();db?.close();});requestAnimationFrame(frame);
 window.wgrFixture={get frames(){return frames},get score(){return score},input,save,restore:()=>({x,y,score})};
})().catch(e=>{document.querySelector('#mode').textContent='Falha: '+e.message;window.chrome?.webview?.postMessage(JSON.stringify({type:'fixture_error',error:e.message}));});
