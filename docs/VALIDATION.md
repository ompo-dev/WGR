# Validação local — 29/10/2026

O backend padrão é nativo: Wasmtime 50, wgpu/Vulkan na RTX 4060 Laptop e CPAL. O binário WASM da engine é executado diretamente. Não há página, iframe ou WebView no caminho padrão. O executável WGR.exe e o auxiliar wgr-rage.exe ficam juntos na raiz do projeto.

## Verificações executadas

- Formatação do workspace e do host nativo: passou.
- Clippy do workspace com todos os alvos e avisos tratados como erro: passou. Permanece o aviso de compatibilidade futura de proc-macro-error2.
- Workspace: 27 testes passaram; 1 teste de rede é ignorado na execução determinística.
- Host nativo: 7 testes passaram, incluindo leitura de mapa de 29.749.248 bytes em blocos limitados, rejeição de destinos inválidos, interpretação da fila antecipada da engine, gesto de Esc, viewport e saves.
- Clippy do fork nativo: executado; permanecem avisos de estilo no código herdado. Não foi declarado livre de avisos.
- Builds release dos dois executáveis: passaram.
- Fixture original Órbita: três execuções passaram, com execução WASM, entrada Win32 e restauração de save. Seus tempos não são um benchmark do GTA.

Evidências: artifacts/native-root-clippy.log, native-root-tests.log, native-root-build.log, native-rage-tests.log, native-rage-clippy.log, native-rage-build.log e native-fixture-final.log.

## GTA no host nativo

As execuções locais chegaram ao mapa, com personagem, controles usados pelo usuário, áudio nativo e apresentação GPU. Isso substitui o registro antigo que tinha parado na tela de carregamento. Não equivale a compatibilidade completa ou estabilidade de FPS em todo o mapa.

A execução 09 reproduziu o fechamento ao redimensionar: a thread de renderização da engine caiu em emscripten_builtin_free durante NullResource::Release / grcResourceCache::UnloadCache / grcDevice::DeviceLost. O host agora conserva a resolução interna durante a sessão e redimensiona somente a superfície de apresentação, com escala proporcional e ajuste das coordenadas do mouse.

Na execução 10, a engine permaneceu viva e apresentou novos quadros em quatro tamanhos de janela: 960×640, 1500×950, 1100×720 e 1280×800. Resultado em artifacts/native-resize-smoke.json. A captura artifacts/native-child-current.png mostra chão, rua, construções e vegetação desenhados na região de Sandy Shores. É uma observação dessa região, não uma garantia para todos os deslocamentos.

O teste sintético completo de teclado/mouse não foi declarado aprovado: o Windows impediu a ativação automática da janela durante a verificação mais recente. O gesto de Esc e a liberação de input ao perder foco têm testes próprios e registros de execução anteriores; a verificação interativa da versão atual depende da janela em primeiro plano.

## Mudanças de carregamento e controles

O host consome a fila de antecipação da própria engine, com prioridade alta antes da baixa, deduplicação e tarefas limitadas. Os contadores hints_queued e hints_completed confirmaram processamento real na execução 10. As leituras efetivas usam um pool limitado a 32 tarefas; arquivos grandes são divididos em trechos de até 1 MiB, sem um segundo buffer do tamanho inteiro do arquivo.

O transporte oferece HTTP/2 com negociação e fallback, janela adaptativa e conexões reutilizadas. A origem respondeu HTTP/2.0 na sondagem registrada em artifacts/origin-http2.json. Os 18 packs de shaders gráficos são preparados antes da renderização.

O cache verifica e entrega os bytes em uma passagem, evitando uma releitura. Seu índice usa WAL/NORMAL; após uma queda de energia, entradas recentes de cache podem ser perdidas e baixadas novamente. Isso se aplica somente ao cache descartável, não aos saves. Referência: https://www.sqlite.org/pragma.html#pragma_synchronous

Um toque em Esc continua indo ao jogo. Dois toques sem repetição automática, separados por até 350 ms, liberam o cursor; o segundo não alterna novamente o menu. Outra tecla ou mudança de foco reinicia o gesto. Alt+Tab libera a captura e as teclas retidas. F11 e Ctrl+Shift+B são encaminhados à janela WGR.

## Limites das medições

O cache quente do transporte permanece em 48 MiB e o orçamento de disco do runtime em até 2 GiB, incluindo código compilado. Buffers transitórios, saves, memória linear da engine e caches do sistema são medidos separadamente. O WASM original exige memória compartilhada de no mínimo 3 GiB; Rust e execução direta não eliminam os dados do jogo.

FPS informado é a contagem de apresentações do host, não uma medição física no monitor. Os contadores Mach herdados que aparecem como zero no log não medem a VRAM do Windows; use os contadores do sistema coletados por Monitor-Gta.ps1.

Permanecem por validar percursos controlados com cache frio, voos rápidos por regiões novas, toda a matriz de DPI/monitores, gamepad físico e escuta humana. Não foi afirmado que todos os travamentos desapareceram. Nenhum repositório de referência foi alterado, nem foram distribuídos assets proprietários do jogo.

Após os testes, os intermediários de compilação foram removidos: 7,27 GiB liberados na medição, com 33,17 GiB livres naquele momento. Executáveis finais, cache persistente do jogo e saves foram mantidos. Evidência: artifacts/native-cleanup-summary.json. A engine continuou apresentando quadros após a limpeza.
