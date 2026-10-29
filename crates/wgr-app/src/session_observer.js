(()=>{
  if(self!==top)return;
  const send=value=>{try{chrome.webview.postMessage(JSON.stringify(value))}catch{}};
  if(location.origin==='https://fiveplay.lol'){
    const OriginalWorker=self.Worker;
    self.Worker=new Proxy(OriginalWorker,{construct(target,args,newTarget){
      const url=new URL(args[0],location.href);
      if(!url.pathname.endsWith('/loader.js'))return Reflect.construct(target,args,newTarget);
      url.searchParams.set('wgr_loader','bounded-v2');
      const worker=Reflect.construct(target,[url.href,...args.slice(1)],newTarget),post=worker.postMessage.bind(worker);
      let module,memory,lastMemory=0;
      worker.addEventListener('message',event=>{memory=event.data?.input?.memory||event.data?.audio?.memory||memory});
      setInterval(()=>{
        if(self.performance?.mark){const mark='wgr-epoch-'+Date.now();performance.mark(mark);performance.clearMarks(mark)}
        const bytes=memory?.buffer.byteLength||0;
        if(bytes!==lastMemory){lastMemory=bytes;send({type:'game-progress',payload:{memory:{linear_bytes:bytes}}})}
      },5000);
      worker.postMessage=(message,transfers)=>{
        if(!message?.canvas)return post(message,transfers);
        const query=new URLSearchParams(location.search),profile=query.get('wgr_quality')==='economy'?'economy':'balanced';
        if(profile==='balanced'){
          const manual=(query.get('args')||'').split(',');let args=[...message.args];
          for(const [name,value] of [['textureQuality','1'],['lodScale','0.5']]){
            const prefix='-'+name+'=',explicit=manual.findLast(arg=>arg.startsWith(prefix));
            args=args.filter(arg=>!arg.startsWith(prefix));args.push(explicit||prefix+value);
          }
          message={...message,args};
        }
        send({type:'game-progress',payload:{configuration:{args:message.args,cores:message.cores,low_memory:message.lowMemory,profile}}});
        const engine=new URL('game.wasm?wgr_native=1',url);
        module??=WebAssembly.compileStreaming(fetch(engine,{cache:'force-cache'}));
        module.then(wgrModule=>post({...message,wgrModule},transfers)).catch(error=>send({type:'game-crash',payload:{error:String(error)}}));
      };
      return worker;
    }});
  }
  if(new URLSearchParams(location.search).get('nocache')==='1'&&navigator.storage?.getDirectory){
    navigator.storage.getDirectory().then(root=>root.removeEntry('gamedata',{recursive:true})).then(()=>send({type:'game_cache_cleared'})).catch(()=>{});
  }
  for(const name of ['game-fps','game-progress','game-crash']){
    const channel=new BroadcastChannel(name);
    channel.onmessage=event=>send({type:name,payload:event.data});
  }
  const gpuLog=new BroadcastChannel('game-log');
  gpuLog.onmessage=event=>{const line=String(event.data);if(line.startsWith('[wgpu]'))send({type:'game-progress',payload:{gpu_log:line.slice(0,4096)}})};
  addEventListener('error',event=>{if(event instanceof ErrorEvent)send({type:'page_error',message:String(event.message).slice(0,2048)})},true);
  addEventListener('unhandledrejection',event=>send({type:'page_error',message:String(event.reason).slice(0,2048)}));
  let last='';
  setInterval(()=>{
    const state=document.documentElement.dataset;
    const value={type:'port_state',label:(document.getElementById('label')?.textContent||'').slice(0,2048),hud:(document.getElementById('hud')?.textContent||'').slice(0,2048),percent:(document.getElementById('pct')?.textContent||'').slice(0,32),start_mode:state.wgrStartMode,input_ready:state.wgrInputReady,mode_committed:state.wgrModeCommitted,map_picker:state.wgrMapPicker};
    const current=JSON.stringify(value);if(current!==last){last=current;send(value)}
  },2000);
})();
