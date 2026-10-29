# Compatibilidade

| Recurso | Implementação / prova |
|---|---|
| GPUI Windows | Janela nativa com render Direct3D, lista virtualizada e formulários reais |
| WebView2 | Perfil por jogo, child HWND, resize/foco/fullscreen; interface de filtros v22 necessária |
| Wasm | Fixture original e módulo memory64 de validação |
| Worker / OffscreenCanvas | Worker real desenha em OffscreenCanvas e confirma isolamento |
| WebGPU | requestAdapter/requestDevice e submissão real na fixture quando disponível |
| Áudio | AudioWorklet carregado; AudioContext ativado por clique nativo no smoke |
| Saves | IndexedDB por origem estável; reabertura testada |
| Gamepad | API detectada e código de input implementado; dispositivo físico requer teste |
| URL pública | HTTPS e allowlist; broker GET/HEAD/POST com limites, sem cookies de páginas/WebSockets |
| Catálogo | Protocolo v1 paginado, teste determinístico de duas páginas |
| GTA | Origem original/CDN, cache persistente limitado e loader verificado; fallback ZIP preservado; execução em VALIDATION.md |

O probe é executado na página controlada de preparação e novamente no documento do jogo. Requisitos compartilhados falham se a origem efetiva não for isolada. WebGPU disponível não garante que todos os limites que uma engine pede caibam na GPU; falhas da engine são reportadas sem contornar isolamento.

## GTA

O padrão usa o site/CDN original identificado pelo HAR. O módulo exige memory64 compartilhada com mínimo de 3 GiB, pthreads/Emscripten e APIs próprias. Servo/Oxide/Blitz ainda não foram integrados como player; a avaliação está em [NATIVE-RUNTIME.md](NATIVE-RUNTIME.md). No fallback, o snapshot ZIP indexa 20.243.587.461 bytes e 12.823 entradas; o build é descoberto pelas chaves `b/`, ignorando diretórios vazios. Nenhum arquivo de engine ou jogo acompanha o pacote WGR. Os resultados concretos ficam em VALIDATION.md.

Mirror local usa sua própria entrada `index.html` e os diretórios `b/`/`data/`. ZIP remoto usa a landing do host e o índice; Archive.org e o CDN verificado estão pré-configurados para a fonte indicada pelo usuário. Outros hosts exigem atualização explícita da lista da fonte. Membros DEFLATE acima dos limites atuais (1 GiB por objeto, expansão 500 vezes) retornam erro explícito. Esse teto e a quota de spool podem exigir evolução para bundles grandes; uma resposta HTTP do loader não comprova jogabilidade.

Não há conversão `.exe → .wasm`, cloud gaming, suporte genérico a APIs arbitrárias, parâmetros GTA aplicados a outros jogos ou sliders universais de FPS/qualidade.
