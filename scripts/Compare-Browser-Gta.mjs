#!/usr/bin/env node
/**
 * Reference instrumentation for the authorized GTA web port.
 * Uses an isolated Chromium profile and the official Chrome DevTools
 * Protocol; neither patches browser internals nor requires extensions.
 *
 * Usage:
 *   node scripts/Compare-Browser-Gta.mjs --seconds 150
 *   node scripts/Compare-Browser-Gta.mjs --seconds 150 --url https://fiveplay.lol/?mode=sandbox
 *
 * No passwords, cookies, request bodies, or browser-profile data are logged.
 */
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const argv = process.argv.slice(2);
const arg = (key, defaultValue) => {
  const pos = argv.indexOf(key);
  return pos < 0 ? defaultValue : argv[pos + 1] ?? defaultValue;
};
const seconds = Math.min(300, Math.max(20, Number(arg("--seconds", "150"))));
const uri = new URL(arg("--url", "https://fiveplay.lol/?mode=sandbox&res=1280x720"));
if (uri.protocol !== "https:" || !["fiveplay.lol", "v2.fiveplay.lol"].includes(uri.hostname)) {
  throw new Error("Only the official HTTPS FivePlay web-port hosts are accepted.");
}
const autoPlay = argv.includes("--play");
const output = resolve(arg("--output", "artifacts/browser-reference-edge"));
const edge = arg("--browser", "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe");
const ownProfile = join(output, "isolated-profile");
await mkdir(output, { recursive: true });

function timeout(promise, limitMs, label) {
  return Promise.race([
    promise,
    sleep(limitMs).then(() => { throw new Error(`Timeout: ${label}`); }),
  ]);
}

class CDP {
  constructor(url) {
    this.pending = new Map();
    this.handlers = new Map();
    this.nextId = 1;
    this.socket = new WebSocket(url);
    this.ready = new Promise((ok, fail) => {
      this.socket.addEventListener("open", ok, { once: true });
      this.socket.addEventListener("error", fail, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      let msg;
      try { msg = JSON.parse(event.data); } catch { return; }
      if (msg.id !== undefined) {
        const pending = this.pending.get(msg.id);
        if (!pending) return;
        this.pending.delete(msg.id);
        if (msg.error) pending.reject(new Error(JSON.stringify(msg.error)));
        else pending.resolve(msg.result ?? {});
      } else if (msg.method) {
        for (const handler of this.handlers.get(msg.method) ?? []) handler(msg.params ?? {});
      }
    });
    this.socket.addEventListener("close", () => {
      for (const pending of this.pending.values()) pending.reject(new Error("CDP closed"));
      this.pending.clear();
    });
  }
  on(method, fn) {
    if (!this.handlers.has(method)) this.handlers.set(method, []);
    this.handlers.get(method).push(fn);
  }
  async call(method, params = {}, limitMs = 15000) {
    await this.ready;
    const id = this.nextId++;
    const promise = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
    this.socket.send(JSON.stringify({ id, method, params }));
    return timeout(promise, limitMs, method);
  }
  close() { try { this.socket.close(); } catch {} }
}

async function availablePort() {
  const server = createServer();
  await new Promise((ok, fail) => server.once("error", fail).listen(0, "127.0.0.1", ok));
  const port = server.address().port;
  await new Promise((ok) => server.close(ok));
  return port;
}

async function getJson(url, attempts = 70) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(1300) });
      if (res.ok) return await res.json();
    } catch {}
    await sleep(300);
  }
  throw new Error(`No CDP response from ${url}`);
}

const report = {
  kind: "wgr-browser-reference-v1",
  started_at: new Date().toISOString(),
  browser: "Microsoft Edge (Chromium)",
  profile: "new and isolated (not the user's personal profile)",
  target_origin: uri.origin,
  target_path: uri.pathname,
  requested_seconds: seconds,
  samples: [],
  screenshots: [],
  network: { requests: 0, bytes: 0, status: {}, mime: {}, failed: [], origins: {} },
  console_errors: [],
  runtime_errors: [],
  warnings: [],
};
async function save() {
  await writeFile(join(output, "reference.json"), JSON.stringify(report, null, 2), "utf8");
}

const port = await availablePort();
const child = spawn(edge, [
  `--remote-debugging-port=${port}`,
  "--remote-debugging-address=127.0.0.1",
  `--user-data-dir=${ownProfile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--window-size=1280,800",
  "about:blank",
], { windowsHide: false, stdio: "ignore" });
let browser;
let page;
try {
  const endpoint = `http://127.0.0.1:${port}`;
  const version = await getJson(endpoint + "/json/version");
  report.browser_version = version.Browser;
  browser = new CDP(version.webSocketDebuggerUrl);
  const { targetId } = await browser.call("Target.createTarget", { url: "about:blank" });
  const targets = await getJson(endpoint + "/json/list");
  const target = targets.find((t) => t.id === targetId);
  if (!target?.webSocketDebuggerUrl) throw new Error("CDP tab target unavailable");
  page = new CDP(target.webSocketDebuggerUrl);
  const requests = new Map();
  page.on("Network.requestWillBeSent", ({ requestId, request }) => {
    let origin;
    try { origin = new URL(request.url).origin; } catch { origin = "internal"; }
    requests.set(requestId, origin);
    report.network.requests++;
    report.network.origins[origin] = (report.network.origins[origin] ?? 0) + 1;
  });
  page.on("Network.responseReceived", ({ requestId, response, type }) => {
    const status = String(response.status);
    const mime = (response.mimeType || type || "unknown").slice(0, 75);
    report.network.status[status] = (report.network.status[status] ?? 0) + 1;
    report.network.mime[mime] = (report.network.mime[mime] ?? 0) + 1;
  });
  page.on("Network.loadingFinished", ({ requestId, encodedDataLength }) => {
    report.network.bytes += encodedDataLength ?? 0;
    requests.delete(requestId);
  });
  page.on("Network.loadingFailed", ({ requestId, errorText, blockedReason }) => {
    if (report.network.failed.length < 100) {
      report.network.failed.push({ origin: requests.get(requestId), reason: blockedReason || errorText });
    }
    requests.delete(requestId);
  });
  page.on("Runtime.exceptionThrown", ({ exceptionDetails }) => {
    if (report.runtime_errors.length < 100) {
      report.runtime_errors.push({
        text: exceptionDetails?.text?.slice(0, 320),
        exception: exceptionDetails?.exception?.description?.slice(0, 400),
      });
    }
  });
  page.on("Log.entryAdded", ({ entry }) => {
    if (entry?.level === "error" && report.console_errors.length < 120) {
      report.console_errors.push({ source: entry.source, text: entry.text?.slice(0, 350) });
    }
  });
  await Promise.all([
    page.call("Page.enable"),
    page.call("Runtime.enable"),
    page.call("Network.enable"),
    page.call("Log.enable"),
    page.call("Performance.enable"),
  ]);
  // Noninvasive inspection: observe the real browser's APIs and resources.
  try {
    const gpu = await browser.call("SystemInfo.getInfo", {}, 10000);
    report.gpu = {
      devices: gpu.gpu?.devices?.map((d) => ({
        vendor: d.vendorString, device: d.deviceString, vendor_id: d.vendorId, device_id: d.deviceId,
      })),
      feature_status: gpu.gpu?.featureStatus,
    };
  } catch (error) { report.warnings.push("GPU system info: " + error.message); }
  await page.call("Page.navigate", { url: uri.href }, 18000);
  if (autoPlay) {
    let launched = false;
    for (let attempt = 0; attempt < 30 && !launched; attempt++) {
      await sleep(500);
      const button = await page.call("Runtime.evaluate", {
        expression: `(() => {
          const button = [...document.querySelectorAll('button')]
            .find(b => /^play\\s*(?:→|$)/i.test(b.innerText.trim()));
          if (!button) return false;
          button.click();
          return true;
        })()`,
        returnByValue: true,
      });
      launched = button.result?.value === true;
    }
    report.play_button_clicked = launched;
    console.log("CHROMIUM_REFERENCE_PLAY_CLICKED", launched);
    if (!launched) report.warnings.push("No visible official Play button was found");
  }
  console.log("CHROMIUM_REFERENCE_STARTED", JSON.stringify({
    browser: report.browser_version, seconds, target: uri.origin + uri.pathname, port,
  }));

  const checkpoints = [10, 30, 75, 120, 180, 240, 300].filter((value) => value <= seconds);
  checkpoints.push(seconds);
  const start = Date.now();
  let previousCheckpoint = 0;
  for (let tick = 0; Date.now() - start < seconds * 1000; tick++) {
    const nowSeconds = Math.round((Date.now() - start) / 1000);
    let metrics = {};
    let dom = {};
    try {
      const result = await page.call("Performance.getMetrics", {}, 13000);
      metrics = Object.fromEntries((result.metrics || []).filter((m) =>
        ["Timestamp", "Documents", "Frames", "JSEventListeners", "Nodes", "LayoutCount",
         "RecalcStyleCount", "ScriptDuration", "TaskDuration", "JSHeapUsedSize",
         "JSHeapTotalSize", "FramesPerSecond"].includes(m.name)
      ).map(({ name, value }) => [name, value]));
      const evaluated = await page.call("Runtime.evaluate", {
        expression: `(() => {
          const entries = performance.getEntriesByType('resource');
          const wasm = entries.filter(r => /\\.wasm(?:\\?|$)/i.test(r.name));
          const canvases = [...document.querySelectorAll('canvas')].map(c => ({
            id: c.id.slice(0,50), width:c.width, height:c.height,
            visible: !!c.getClientRects().length
          }));
          const bodyText = document.body?.innerText || '';
          return {
            ready: document.readyState, title: document.title.slice(0,100),
            canvases, iframes: document.querySelectorAll('iframe').length,
            resource_count: entries.length,
            wasm_resources: wasm.map(r=>({name:new URL(r.name).pathname, duration_ms:Math.round(r.duration),
              transfer_bytes:r.transferSize, decoded_bytes:r.decodedBodySize})).slice(0,12),
            body_status: bodyText.slice(0,250),
            webgpu_exposed:!!navigator.gpu,
          };
        })()`,
        returnByValue: true,
        awaitPromise: false,
      }, 13000);
      dom = evaluated.result?.value ?? {};
    } catch (error) {
      report.warnings.push(`sample ${tick}: ${error.message}`);
    }
    report.samples.push({
      elapsed_seconds: nowSeconds,
      ...metrics,
      page: dom,
      network_bytes: report.network.bytes,
      requests: report.network.requests,
    });
    const checkpoint = checkpoints.find((time) =>
      time <= nowSeconds + 3 && time > previousCheckpoint);
    if (checkpoint !== undefined) {
      try {
        const screenshot = await page.call("Page.captureScreenshot",
          { format: "jpeg", quality: 82, fromSurface: true, captureBeyondViewport: false }, 20000);
        if (screenshot.data) {
          const filename = `browser-${String(checkpoint).padStart(3, "0")}s.jpg`;
          await writeFile(join(output, filename), Buffer.from(screenshot.data, "base64"));
          report.screenshots.push({ seconds: nowSeconds, filename });
        }
      } catch (error) { report.warnings.push("screenshot: " + error.message); }
      previousCheckpoint = checkpoint;
      await save();
      console.log("CHROMIUM_REFERENCE_CHECKPOINT", JSON.stringify({
        seconds: nowSeconds, canvases: dom.canvases?.length,
        resource_count: dom.resource_count, wasm: dom.wasm_resources?.length,
        js_heap_mib: Math.round((metrics.JSHeapUsedSize ?? 0) / 1048576),
        network_mib: Math.round(report.network.bytes / 1048576),
        errors: report.runtime_errors.length + report.console_errors.length,
      }));
    }
    await sleep(Math.min(5000, Math.max(0, seconds * 1000 - (Date.now() - start))));
  }
  report.finished_at = new Date().toISOString();
  report.success = true;
  await save();
  console.log("CHROMIUM_REFERENCE_FINISHED", join(output, "reference.json"));
} catch (error) {
  report.success = false;
  report.failure = String(error.stack || error);
  await save();
  console.error("CHROMIUM_REFERENCE_FAILED", error.message);
  process.exitCode = 1;
} finally {
  try { await browser?.call("Browser.close", {}, 4000); } catch {}
  page?.close();
  browser?.close();
  if (child?.pid) {
    await sleep(500);
    try { child.kill(); } catch {}
  }
}
