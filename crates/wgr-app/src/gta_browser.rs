//! Adapt only the verified loader: a compiled module crosses the worker boundary,
//! while the bounded IO worker runs from a same-origin Blob.
use anyhow::{Result, ensure};

pub fn loader(loader: &[u8], io: &[u8], gpu: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        wgr_core::assets::sha256(loader)
            == "9de25898fa9a9ec5e67ca647debf941849b936e3163402944581feaaae5ac0ff",
        "Loader da origem mudou; adaptação interrompida"
    );
    ensure!(
        wgr_core::assets::sha256(io)
            == "d91c89e64f7f280666b5c9d1669ed6d65d66605077264569a49146ed79fe0bae",
        "Worker IO da origem mudou; adaptação interrompida"
    );
    ensure!(
        wgr_core::assets::sha256(gpu)
            == "826dfaa9012dee081db6985b8973c18882ccafae23f935f6eb18eec392ebec58",
        "Worker GPU da origem mudou; adaptação interrompida"
    );
    let io = format!(
        "'use strict';self.WGR_PERSISTENT_CACHE=true;\n{}\n{}",
        std::str::from_utf8(io)?,
        wgr_http::RAM_IO_SCRIPT
    );
    let gpu = gpu_patch(std::str::from_utf8(gpu)?)?;
    patch(std::str::from_utf8(loader)?, &io, &gpu)
}
fn gpu_patch(gpu: &str) -> Result<String> {
    let frame = "const dt = tn - lastPresentT;";
    let fps = "fpsOverlayN = 0; fpsOverlayT = tn;";
    let readback = "const done = () => { if (flagAddr)";
    let fence = "await device.queue.onSubmittedWorkDone();";
    let query = "function mapResolves(res) {";
    let query_result = "const v = new BigUint64Array(res.map.getMappedRange());";
    ensure!(
        gpu.matches(frame).count() == 1
            && gpu.matches(fps).count() == 1
            && gpu.matches(readback).count() == 1
            && gpu.matches(fence).count() == 1
            && gpu.matches(query).count() == 1
            && gpu.matches(query_result).count() == 1,
        "Contrato de diagnóstico GPU incompatível"
    );
    Ok(format!(
        "{}\n{}",
        gpu.replace("self.location.search", "wgrGpuQuery")
            .replacen(frame, &format!("{frame}\nwgrRecordFrame(dt,tn);"), 1)
            .replacen(fps, &format!("{fps}\nwgrReportGpu(tn);"), 1)
            .replacen(readback,"const wgrReadbackT=performance.now();const done = () => { wgrGpuWait('readback',wgrReadbackT);if (flagAddr)",1)
            .replacen(fence,&format!("const wgrFenceT=performance.now();{fence}wgrGpuWait('fence',wgrFenceT);"),1)
            .replacen(query,&format!("{query}const wgrQueryT=performance.now();"),1)
            .replacen(query_result,&format!("wgrGpuWait('occlusion',wgrQueryT);{query_result}"),1)
            .replacen("for (const rc of local.concat(seed)) {", "for (const rc of local.slice(-320).reverse().concat(seed,local.slice(0,-320).reverse())) {",1)
            .replacen("aheadList.push([rc, key]);", "if(aheadList.length<512) aheadList.push([rc, key]);",1),
        include_str!("gpu_observer.js")
    ))
}
fn patch(loader: &str, io: &str, gpu: &str) -> Result<Vec<u8>> {
    let worker = "const io = new Worker(B + '/io_worker.js');";
    let instantiate = "instantiateWasm: (imports, done) => {";
    let gpu_log = "if (m.remoteLog) gq.set('log', '1');";
    let gpu_worker =
        "const gpu = new Worker(B + '/wgpu_worker.js' + (gq.toString() ? '?' + gq : ''));";
    let shader_url = "wgpuShaderUrl: B + '/shaders',";
    ensure!(
        loader.matches(worker).count() == 1
            && loader.matches(instantiate).count() == 1
            && loader.matches(gpu_log).count() == 1
            && loader.matches(gpu_worker).count() == 1
            && loader.matches(shader_url).count() == 1,
        "Contrato do loader incompatível"
    );
    let replacement = format!(
        "const io = new Worker(URL.createObjectURL(new Blob([{}], {{type:'text/javascript'}})));",
        serde_json::to_string(io)?
    );
    let loader = loader.replacen(worker, &replacement, 1).replacen(instantiate, &format!("{instantiate}\nif(m.wgrModule){{WebAssembly.instantiate(m.wgrModule,imports).then(instance=>{{bc.postMessage({{pct:22,label:'Starting the engine'}});done(instance,m.wgrModule);}}).catch(e=>bc.postMessage({{error:String(e)}}));return {{}};}}"), 1);
    // Only the GPU broadcasts locally. The document's remoteLog stays false,
    // so no diagnostic POST is sent to the site and engine key logs stay off.
    let gpu_replacement = format!(
        "const gpu = new Worker(URL.createObjectURL(new Blob(['\"use strict\";const wgrGpuQuery='+JSON.stringify('?'+gq)+';\\n',{}],{{type:'text/javascript'}})));",
        serde_json::to_string(gpu)?
    );
    Ok(loader
        .replacen(gpu_log, "gq.set('log', '1');", 1)
        .replacen(gpu_worker, &gpu_replacement, 1)
        .replacen(
            shader_url,
            "wgpuShaderUrl: new URL(B + '/shaders',self.location.origin).href,",
            1,
        )
        .into_bytes())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_loader_is_rejected_and_worker_source_is_escaped() {
        assert!(loader(b"unknown", b"unknown", b"unknown").is_err());
        let fixture = "if (m.remoteLog) gq.set('log', '1'); const gpu = new Worker(B + '/wgpu_worker.js' + (gq.toString() ? '?' + gq : '')); const io = new Worker(B + '/io_worker.js'); const Module={instantiateWasm: (imports, done) => {return {};},wgpuShaderUrl: B + '/shaders',};";
        let result =
            String::from_utf8(patch(fixture, "'line'\n\"quoted\"", "//gpu").expect("patch"))
                .expect("utf8");
        assert!(result.contains("m.wgrModule"));
        assert!(result.contains("URL.createObjectURL"));
        assert!(!result.contains("if (m.remoteLog) gq.set"));
        assert!(result.contains("\\n\\\"quoted\\\""));
        assert!(result.contains("wgrGpuQuery"));
        assert!(result.contains("self.location.origin).href"));
        assert!(patch("other contract", "", "").is_err());
        assert!(gpu_patch("other contract").is_err());
    }
}
