# Player nativo — reescrita Rust baseada no 5-web-port

O player nativo substitui o WebView2 executando a mesma engine WASM em
Wasmtime, com GPU wgpu, entrada Win32, áudio CPAL e leitura de assets sob
demanda. A referência funcional é o 5-web-port (`public/index.html`,
`public/play.html`, `loader.js`, `wgpu_worker.js`, `io_worker.js`), que roda a
mesma build `8b0b5899ed`.

A reescrita vive em `crates/wgr-player` e reaproveita os contratos já testados
do workspace:

- `wgr-runtime`: memória compartilhada memory64, ring de comandos da GPU
  (`gpu_protocol`) e carregamento Wasmtime.
- `wgr-http`/`wgr-core`: servidor de assets, cache em disco e descritor do port.
- `wgr-player`: o player em si (novo).

## Mapa dos arquivos de referência

| 5-web-port | wgr-player | Estado |
| --- | --- | --- |
| `play.html` (bloco de input, viewport, `wantSize`) | `input.rs` | portado e testado |
| `wgpu_worker.js` (`packUniforms`, formatos, ring) | `shader.rs` + futuros `gpu/*` | empacotamento portado; backend em andamento |
| `play.html` (argumentos da engine) | `loader.rs` | portado e testado |
| `loader.js` (módulo/instância) | a definir | usa `wgr-runtime` |
| `io_worker.js` (hints/leituras) | a definir | usa o cache de `wgr-core` |
| `audio-worklet.js` | a definir | CPAL |
| `server.js` | `wgr-http` | existente |

## Contratos já fixados

- Bloco de input: 256 bytes de teclas, dez palavras i32 (`mouseX`, `mouseY`,
  `dX`, `dY`, `dZ`, botões, foco, capturado, cabeça e cauda), 64 u16 de texto,
  extensão (`winW`, `winH`) e `startMode`. O host escreve com operações
  atômicas; a engine lê direto da memória compartilhada.
- Mouse capturado: `dX/dY` acumulam o movimento relativo e `mouseX/mouseY`
  acompanham com clamp no tamanho do buffer. Sem captura, a posição absoluta é
  convertida em delta, como no `mousemove` do play.html.
- Uniformes: todos os `var<uniform>` de um grupo viram um `cbpack_t` no
  binding 0, com offset dinâmico; o binding 15 permanece inline.
- Argumentos: `-forceResolution`, `-width/-height` iguais ao buffer interno,
  `-fastscriptcore`, `-frameLimit`, `-numCores` apenas com dois núcleos ou
  menos, e o perfil de pouca memória do play.html quando solicitado.
- Tela: buffer interno em pixels físicos (por padrão o tamanho do monitor),
  apresentado com letterbox e mapeamento de mouse pelo mesmo viewport.

## Próximas etapas

1. `gpu/formats.rs`: tabela DXGI → `wgpu::TextureFormat` com `view_formats`.
2. `gpu/resources.rs`: tabela de objetos (texturas, buffers, SRV/UAV, samplers)
   com criação, atualização e destruição seguras.
3. `gpu/pipeline.rs`: cache de pipelines, receitas e compilação assíncrona.
4. `gpu/bind.rs`: bind groups com chave por ids e época de destruição, como o
   worker web.
5. `gpu/pass.rs` e `gpu/draw.rs`: passes, MRT, clears dobrados, viewport e
   estado por desenho.
6. `gpu/readback.rs`: cópia para staging, `map_async` e resposta sem bloquear o
   quadro.
7. `window.rs`, `engine.rs`, `io.rs`, `audio.rs`: hospedar a engine, ler assets
   e apresentar o quadro.
8. Cutover: `wgr-player.exe` substitui `wgr-rage.exe` no launcher, com
   `wgr-rage` mantido como fallback até a paridade.

## Regras

- Nada de WebView, HTML, JavaScript ou iframe no caminho padrão.
- Flags que desativam a segurança do WebView2 não são usadas (o WebView nem é
  carregado).
- GTA só é chamado de jogável com frame, input e bundle autorizado verificados.
- Testes determinísticos usam fixtures originais e memória simulada; o smoke
  real exige GPU e desktop interativo.
