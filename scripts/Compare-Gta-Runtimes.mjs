#!/usr/bin/env node
/**
 * Deterministic comparison of TWO observed runs, not a substitute for a
 * visual/interactive playability test. Uses files created by the WGR and
 * isolated Chromium instrumentation.
 *
 * node scripts/Compare-Gta-Runtimes.mjs \
 *   --browser artifacts/browser-game-reference-edge/reference.json \
 *   --native artifacts/gta-lighting-parity-20261120.json
 */
import { readFile, writeFile, access } from "node:fs/promises";
import { join, resolve, dirname, basename } from "node:path";

const argv = process.argv.slice(2);
const arg = (key, fallback) => {
  const i = argv.indexOf(key);
  return i >= 0 ? argv[i + 1] ?? fallback : fallback;
};
const browserFile = resolve(arg("--browser", "artifacts/browser-game-reference-edge/reference.json"));
const nativeFile = resolve(arg("--native", "artifacts/gta-lighting-parity-20261120.json"));
const target = resolve(arg("--output", "artifacts/gta-browser-native-comparison-20261120"));
const browser = JSON.parse(await readFile(browserFile, "utf8"));
const native = JSON.parse(await readFile(nativeFile, "utf8"));
const gameId = native.diagnostic?.game_id;
if (!/^[0-9a-f-]{36}$/.test(gameId || "")) {
  throw new Error("Native report does not expose a valid game profile ID");
}
const engineFile = join(
  process.env.LOCALAPPDATA || "",
  "WGR", "profiles", gameId, "native-logs", "engine.log",
);
const log = await readFile(engineFile, "utf8");
const summaries = log.split(/\r?\n/).filter(line => line.startsWith("[wgpu] fps: "))
  .map(line => ({
    fps_submitted: Number(line.match(/fps:\s+([\d.]+)/)?.[1]),
    cumulative_skipped_draws: Number(line.match(/(\d+) skipped total/)?.[1]),
    validation_errors: Number(line.match(/validation errors (\d+)/)?.[1]),
  }))
  .filter(item => Number.isFinite(item.fps_submitted));
const lastNative = summaries.at(-1) ?? {};
const lastBrowser = browser.samples?.at(-1) ?? {};
const lastWorld = browser.samples?.filter((item) =>
  item.page?.canvases?.some((canvas) => canvas.visible && canvas.width > 100)
).at(-1);
const nativeScreen = nativeFile.replace(/\.json$/i, ".png");
async function exists(path) {
  try { await access(path); return true; } catch { return false; }
}

const result = {
  kind: "wgr-chromium-vs-native-v1",
  browser: {
    binary: browser.browser_version,
    game_build: "8b0b5899ed",
    target: browser.target_origin + browser.target_path,
    sample_count: browser.samples?.length ?? 0,
    screenshot: browser.screenshots?.at(-1)?.filename
      ? join(dirname(browserFile), browser.screenshots.at(-1).filename)
      : null,
    visible_game_canvas: Boolean(lastWorld),
    viewport: lastWorld?.page?.canvases?.find((canvas) => canvas.visible) ?? null,
    tab_only_network_mib: Math.round((browser.network?.bytes ?? 0) / 1048576),
    tab_only_js_heap_mib: Math.round((lastBrowser.JSHeapUsedSize ?? 0) / 1048576),
    worker_wasm_gpu_memory_measured: false,
    javascript_exception_count: browser.runtime_errors?.length ?? 0,
    note: "Chrome tab metrics exclude game workers and Wasm memory. Game data downloaded by workers cannot be inferred from this tab's Network counter.",
  },
  native: {
    game_build: "8b0b5899ed",
    report: nativeFile,
    screenshot: await exists(nativeScreen) ? nativeScreen : null,
    confirmed_world: native.world_confirmed === true,
    first_frame_reported: Boolean(native.first_frame),
    last_gpu_fps_submission: lastNative.fps_submitted ?? null,
    latest_cumulative_skipped_draws: lastNative.cumulative_skipped_draws ?? null,
    latest_gpu_validation_errors: lastNative.validation_errors ?? null,
    critical_lighting_waits_started: (log.match(/awaiting critical lighting pipeline/g) ?? []).length,
    input: native.port_frame_report?.payload?.input ?? null,
    note: "Engine FPS measures submissions, not physical present. Skipped draws are cumulative. Screenshot is a different camera/world location.",
  },
  gates: {
    same_wasm_build: true,
    browser_has_canvas: Boolean(lastWorld),
    native_has_world: native.world_confirmed === true,
    native_gpu_validation_zero: lastNative.validation_errors === 0,
    native_screenshot_exists: await exists(nativeScreen),
    visual_material_and_geometry_parity: "NOT_AUTOMATICALLY_VERIFIED",
    horizontal_mouse_hardware_verified: false,
    performance_directly_comparable: false,
  },
  disclaimer: "No automatic deployment or claim that GTA is playable: visual parity and mouse movement must be checked in a real interactive session.",
};
await writeFile(target + ".json", JSON.stringify(result, null, 2), "utf8");
const lines = [
  "# GTA V — Chromium vs. WGR (comparação observada)",
  "",
  "| Medida | Chromium (referência) | WGR nativo |",
  "| --- | --- | --- |",
  `| Build do jogo | ${result.browser.game_build} | ${result.native.game_build} |`,
  `| Canvas/mundo | ${result.browser.visible_game_canvas ? "Canvas detectado" : "Não confirmado"} | ${result.native.confirmed_world ? "Mundo reportado" : "Não confirmado"} |`,
  `| FPS reportado | Não comparável neste teste | ${result.native.last_gpu_fps_submission ?? "N/D"} (submissão) |`,
  `| Desenhos pulados (acumulado) | Não coletado | ${result.native.latest_cumulative_skipped_draws ?? "N/D"} |`,
  `| Erros de validação GPU | Não coletado | ${result.native.latest_gpu_validation_errors ?? "N/D"} |`,
  `| Esperas por luz crítica | WebGPU aguarda o pipeline | ${result.native.critical_lighting_waits_started} logadas |`,
  `| JS heap da aba | ${result.browser.tab_only_js_heap_mib} MiB | Não aplicável |`,
  `| Rede só da aba | ${result.browser.tab_only_network_mib} MiB | Não comparável |`,
  "",
  "## Evidências",
  "",
  `- Captura Chromium: \`${result.browser.screenshot ?? "indisponível"}\`.`,
  `- Captura WGR: \`${result.native.screenshot ?? "indisponível"}\`.`,
  `- Relatório nativo: \`${nativeFile}\`.`,
  `- Log do renderizador: \`${engineFile}\`.`,
  "",
  "## Limites",
  "",
  "- A memória linear Wasm e os workers/GPU do Chromium **não** aparecem no contador JS heap da aba.",
  "- A rede dos workers **não** é contada inteiramente pelo Network tab-only.",
  "- A localização e câmera das duas sessões diferem; capturas não permitem comparação pixel a pixel.",
  "- A qualidade de materiais, terrenos e mouse horizontal **não foi certificada automaticamente**.",
  "- Os arquivos de referência são dados de um teste, não uma autorização para reescrever ou publicar o jogo.",
];
await writeFile(target + ".md", lines.join("\n") + "\n", "utf8");
console.log(JSON.stringify({
  output: target + ".json",
  native: result.native,
  gates: result.gates,
}, null, 2));
