(async()=>{const result={};
const check=async(name,fn)=>{try{result[name]=await fn();}catch(e){result[name]={available:false,error:String(e.message).slice(0,180)};}};
await check('WebAssembly',async()=>{const m=await WebAssembly.instantiate(new Uint8Array([0,97,115,109,1,0,0,0]));return !!m.instance;});
await check('memory64',()=>WebAssembly.validate(new Uint8Array([0,97,115,109,1,0,0,0,5,3,1,4,0])));
result.SharedArrayBuffer=typeof SharedArrayBuffer==='function';result.crossOriginIsolated=crossOriginIsolated;result.Atomics=typeof Atomics==='object';
result.Worker=typeof Worker==='function';result.OffscreenCanvas=typeof OffscreenCanvas==='function';result.transferControlToOffscreen=typeof HTMLCanvasElement.prototype.transferControlToOffscreen==='function';
await check('WebGPU',async()=>{if(!navigator.gpu)return false;const a=await navigator.gpu.requestAdapter();if(!a)return false;const limits={};for(const k of ['maxBufferSize','maxStorageBufferBindingSize','maxComputeWorkgroupStorageSize','maxStorageBuffersPerShaderStage','maxBindGroups'])limits[k]=a.limits[k];const d=await a.requestDevice();d.destroy();return {available:true,adapter:a.info?{vendor:a.info.vendor,architecture:a.info.architecture,device:a.info.device,description:a.info.description}:null,limits};});
await check('AudioWorklet',()=>{const A=window.AudioContext||window.webkitAudioContext;if(!A)return false;return 'audioWorklet' in A.prototype;});
await check('IndexedDB',()=>new Promise(resolve=>{const r=indexedDB.open('wgr-capability-probe',1);r.onsuccess=()=>{r.result.close();resolve(true);};r.onerror=()=>resolve(false);}));
await check('storage',async()=>navigator.storage?.estimate?await navigator.storage.estimate():null);
result.Gamepad=typeof navigator.getGamepads==='function';result.codecs={h264:!!document.createElement('video').canPlayType('video/mp4; codecs="avc1.42E01E"'),opus:!!document.createElement('audio').canPlayType('audio/ogg; codecs="opus"')};
return result;})()
