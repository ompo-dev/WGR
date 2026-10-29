# Execução sem Chromium: contrato e candidatos

Atualização em 29/10/2026. O backend padrão executa binários WASM com Wasmtime 50, sem WebView2. A fixture original Órbita já comprovou execução, entrada e save no host Rust/GPUI. O adaptador GTA nativo já apresentou cenários e recebeu controles em teste local, permanecendo em estabilização, reutilizando [Muguet](https://github.com/c22dev/muguet) no commit `728c195ab02c746a1f2d626238bfe030b2f561fb`, adaptado para Windows. **Compilação não comprova GTA jogável nem melhora de desempenho; a validação de cena, áudio e entrada permanece obrigatória.**

O host `wgr-rage.exe` usa Wasmtime, wgpu nativo/DirectX ou Vulkan, CPAL e threads do sistema. A superfície gráfica é um HWND filho da janela WGR; a barra e os componentes Momor permanecem no launcher. O protocolo GPU compartilhado é interpretado em Rust. Não há navegação de página, iframe ou fallback automático para navegador. O binário da engine é validado por SHA-256 antes da compilação. Os dados são lidos por intervalos via `HttpClient`/`DiskCache` existentes, com cache quente de 48 MiB e orçamento de disco de até 2 GiB incluindo o código compilado. Buffers temporários de leitura e memória da engine são adicionais ao cache quente.

O suporte depende do ABI do port. Um `.wasm` Emscripten não é automaticamente WASI, nem um executável PE/ELF. Outros contratos exigem adaptadores e devem falhar explicitamente quando não implementados. A feature Cargo `web-compat` mantém o backend anterior apenas para diagnóstico explícito; não é parte do player padrão.

## Contrato medido

`node scripts/Audit-Wasm.mjs <game.wasm>` inspeciona o binário sem instanciar a engine. O arquivo auditado tem 63.201.802 bytes e SHA-256 `11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0`. Resultado local em `artifacts/wasm-audit.json`:

- 85 funções importadas de `env`/`wasi_snapshot_preview1`, além de `env.memory`.
- Memória **memory64 compartilhada**, mínimo de **3.221.225.472 bytes (3 GiB)** e máximo declarado de 16 GiB. O máximo não é uma medição de consumo. Memória linear, compromisso e páginas residentes são medidas diferentes.
- ABI Emscripten: pthreads, TLS, mailboxes, proxy de chamadas à thread principal, trampolins `invoke_*` e longjmp; não é um módulo WASI independente.
- APIs próprias: `wgpu_start_worker`, `wasm_httpfs_*`, `wasm_audio_publish_js`, `wasm_input_publish_js` e persistência `wasm_userdata_*`.
- O protocolo GPU usa comandos/fences numa ring buffer compartilhada. O wrapper web original depende de OffscreenCanvas/Worker; o adaptador nativo consome o protocolo diretamente e apresenta uma surface wgpu no HWND.

O `game.js` verificado usa `INITIAL_MEMORY=3221225472`. Reduzir apenas essa constante viola o mínimo importado do WASM. O launcher não copia essa memória a cada mensagem: os workers compartilham o mesmo objeto. Não somar essa região como se cada thread possuísse outra cópia independente.

O `index.html` passa `-frameLimit=1` por padrão (cerca de 60 FPS). `fps=0` seleciona `-frameLimit=0`; o WGR aplica essa opção quando a entrada não tem uma preferência explícita. Isso retira um teto, sem prometer uma taxa de quadros nem alterar o binário. O monitor local foi identificado a 165 Hz. O FPS transmitido pelo worker conta submissões do port, não apresentação física no monitor.

## Projetos indicados pelo usuário

| Base | Evidência primária consultada | Adequação ao binário atual |
| --- | --- | --- |
| Servo | [OffscreenCanvas.webidl v0.7.0](https://github.com/servo/servo/blob/v0.7.0/components/script_bindings/webidls/OffscreenCanvas.webidl), blob `049f4847648b11a08352d1f177a277b52e1dcd8b`; [WebGPU](https://book.servo.org/design-documentation/webgpu.html) | A enumeração do contexto OffscreenCanvas não inclui `webgpu`, embora WebGPU exista em outra parte do motor. Bloqueio estático para o caminho usado por este port; ainda não foi executado um teste GTA em Servo. |
| Oxide | [engine.rs](https://github.com/niklabh/oxide/blob/main/oxide-browser/src/engine.rs), blob `7046ee3913e97d9e472c70aa1c03747e00190212`; [runtime.rs](https://github.com/niklabh/oxide/blob/main/oxide-browser/src/runtime.rs) | Boa referência de host Wasmtime + GPUI. Sua memória é criada com `MemoryType::new`, limite padrão 256 MiB, ABI `oxide::*`, entrada `start_app`/`on_frame`. GTA exige outro contrato, memória64 compartilhada e 3 GiB mínimos. Aumentar a quota não implementa as APIs ausentes. |
| Blitz | [README oficial](https://github.com/DioxusLabs/blitz#goals), blob `f789657b339087be4b8d5e1b4a87f10635c2cf94` | Renderizador HTML/CSS; a documentação não oferece a plataforma de browser necessária. Substituir a UI GPUI/Momor por ele não resolve a execução da engine. |

As conclusões acima são inspeção de código e contrato, **não benchmarks comparando esses motores**. Não foram baixadas distribuições volumosas nem compilados os três projetos. Os outros projetos da lista do usuário não foram certificados como compatíveis. Nenhum ganho de RAM/FPS é inferido apenas da linguagem de implementação.

## Caminho técnico para um host Rust

Sem a fonte da engine, o caminho é hospedar o WASM existente. [Wasmtime](https://docs.wasmtime.dev/stability-wasm-proposals.html) fornece execução WASM e [linkagem explícita das importações](https://docs.wasmtime.dev/api/wasmtime/struct.Linker.html); não implementa automaticamente este ambiente Emscripten. [wgpu](https://github.com/gfx-rs/wgpu) fornece a GPU nativa, mas não interpreta o protocolo particular da engine sozinho. O runner Emscripten do [Wasmer foi removido na versão 5](https://docs.wasmer.io/runtime/runners/emscripten/); não é um atalho atual.

As fronteiras necessárias são:

1. Host Wasmtime com memory64/shared memory, ABI Emscripten, threads/TLS, esperas e encerramento de todos os workers. Importações desconhecidas devem falhar; retornar zero silenciosamente produziria falsa compatibilidade.
2. Consumidor Rust do protocolo GPU existente: recursos, shaders, pipelines, draw/compute, readbacks, queries e fences. Surface própria no HWND do player, sem readback da imagem a cada quadro, filas limitadas e sincronização correta com a GPU.
3. Servidor/cache Rust reaproveitados no filesystem da engine; saves isolados e gravação atômica; áudio pelo sistema e entrada Win32/gamepad com os blocos de memória que o binário espera.
4. Integração na UI Momor já existente. O host precisa provar cena, input, áudio e encerramento, depois ser comparado ao mesmo cenário com resolução e cache iguais. Só então substitui o player padrão.

As implementações estão em `crates/wgr-runtime`, `crates/wgr-app/src/native_*` e `vendor/muguet`. A integração GTA está em validação. O host é escrito em Rust; o binário do jogo e bibliotecas nativas como Opus não foram reescritos em Rust. A fonte autorizada da engine permitiria recompilá-la; os repositórios fornecidos não a contêm.

## Medição histórica do backend web

`scripts/Benchmark-Gta.ps1 -Fps 60 -Name baseline` e, depois, `-Fps 0 -Name uncapped` executam uma engine por vez no mesmo perfil. O harness aguarda o evento de mundo, mede 30 segundos e registra FPS, configurações efetivas, memória linear e caches. O script mede somente a árvore de processos desse WGR, gravando working set, memória privada e espaço livre a cada cinco segundos.

Comparar picos de working set entre processos requer cuidado com páginas compartilhadas. O benchmark não altera a política de memória virtual do Windows e não fornece GPU timestamps, latência de input ou apresentação física; essas métricas exigem captura própria posterior.

