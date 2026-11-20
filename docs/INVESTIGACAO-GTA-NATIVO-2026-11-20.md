# WGR / GTA — investigação de qualidade gráfica, mouse e memória

Data: 20/11/2026. Escopo: `vendor/muguet` (`wgr-rage.exe`), o runtime
**realmente acionado pelo `WGR.exe`** para o port GTA. A fixture Órbita e o
novo `wgr-player` não substituem esse adaptador na versão atual.

## Hipóteses e evidências

| Sintoma | Evidência disponível | Diagnóstico atual |
| --- | --- | --- |
| Pedestres/personagens sem textura | `artifacts/native-baseline-07-engine.log`: `p_head_diff_010_a`, `p_head_diff_027_a` não encontrados; `native-flight-10-engine.log`: `P_HEAD_DIFF_006_A` | **Ao menos alguns recursos faltam no carregamento da própria engine.** Não confundir com textura GPU incorreta: precisamos também verificar quais SRVs e views dos shaders de ped foram vinculados. |
| Mouse horizontal não gira câmera | `artifacts/auto-input-camera-new.json`: orientação não mudou, contadores de mouse zero naquele experimento; `input-live-2.json`: movimentos X/Y realmente chegaram em outra sessão | Falha intermitente de captura/entrada versus lógica da câmera, ainda não identificada como causa única. |
| Memória RAM elevada | `artifacts/perf-ped-input-diagnostic-2-summary.json`: ~6,0 GiB privados e ~4,3 GiB working set no pico; relatório de renderização informa heap do jogo de 3072 MiB | Uma parte é memória linear Wasm usada ativamente por CPU/pthreads: não é elegível a migrar automaticamente para VRAM. Outra parte são texturas/buffers, transcodificações e buffers de upload. |
| Quedas de FPS | `artifacts/input-live-2.json` registrou queda de ~83 para ~31 FPS reportados pela engine | FPS de submissão do port, não medição independente do monitor. Necessário correlacionar streaming, compilações, pipeline e recursos do sistema. |

## Comparação técnica: Servo, MozJS, Surfman e WebRender

- O **Servo** contém infraestrutura completa de navegador (DOM, JS/SpiderMonkey,
  compilador Wasm, eventos, compositor). O jogo nativo WGR já executa Wasm
  diretamente em Wasmtime; adicionar Servo/MozJS introduziria uma segunda pilha
  sem consertar shaders, assets ou câmera.
- **Surfman** separa contexto/superfície da apresentação e cuida de compartilhamento
  de texturas OpenGL. No WGR, a superfície nativa já é criada por winit/wgpu,
  portanto **não** há razão para adicionar Surfman apenas para fazer a GPU funcionar.
- **WebRender** acelera pintura/composição de páginas 2D via OpenGL; não
  implementa a tradução do protocolo gráfico do jogo para as texturas, samplers,
  buffers uniformes e pipelines exigidos.
- **wgpu 30** já seleciona **Vulkan ou Direct3D 12 diretamente** no Windows,
  sem um serviço de navegador. Substituí-lo por OpenGL puro teria alto custo de
  reimplementação e não comprova menor uso de RAM ou melhor compatibilidade.
- Transferir texturas e render targets para a memória dedicada da GPU é
  responsabilidade da camada de recursos do driver. A memória compartilhada
  Wasm de `memory64` continua disponível para CPU, threads e jogo. Reduzir
  duplicações de upload é uma otimização real, mas não converte automaticamente
  os 3 GiB de heap em VRAM.

Fontes e referências:

- https://book.servo.org/design-documentation/architecture.html
- https://github.com/servo/surfman
- https://github.com/servo/webrender
- https://wgpu.rs/doc/wgpu/index.html
- Repositórios locais de referência: `webrender`, `surfman`, `mozjs` — sem modificações.

## Alterações aplicadas nesta etapa

1. **Mouse:** em Windows com cursor capturado, tanto `Locked` quanto
   `Confined`, se o SO deixa de entregar eventos relativos por mais de
   250 ms, o host usa deslocamento da posição visual em relação ao centro e
   recentraliza. Eventos relativos permanecem prioritários; `CursorMoved`
   não é duplicado quando os eventos raw chegam.
   Teste unitário cobre X, Y, centro e janela sem tamanho.
2. **Diagnóstico de textura:** na ausência de uma SRV válida para um shader de
   personagem, a engine registra uma vez por programa/binding a causa
   (`nao vinculado`, `inexistente`, `aponta para buffer` ou
   `view/formato incompativel`). Ativado com `WGR.exe --gta-logs` via
   `MUGUET_TEXTURE_DIAGNOSTICS=1`. O log nativo fica em
   `<dados WGR>/profiles/<id>/native-logs/engine.log`; não grava conteúdo de
   texturas nem captura dados privados.
3. **Uploads:** as texturas que não precisam de descompressão ou conversão de
   formato são passadas do endereço Wasm diretamente para o staging do
   `Queue::write_texture`, removendo um `Vec` temporário por operação.
   Conversões BC, half e alpha ainda usam memória própria.
4. **Cache de materiais:** o ID de uma view, sampler ou textura não é a sua
   identidade completa: a engine pode destruí-lo e recriar outro recurso com
   o mesmo número. Os *bind groups* agora incorporam uma geração por recurso
   para evitar que uma entrada do cache reutilize uma textura antiga quando
   o jogo recicla IDs. O mecanismo inclui um teste de regressão de reciclagem.
   O cache ainda retém entradas antigas até sua política de limpeza; **não
   afirmar redução de VRAM sem medir**.
5. **Medição de VRAM:** o contador `gpu 0 MB` do runtime Windows era um
   *stub* sem leitura real, não indicava ausência de renderização pela GPU.
   O FPS log passa a mostrar `gpu não medido`; o script
   `scripts/Benchmark-Gta.ps1` resume os picos de memória GPU dedicada,
   compartilhada e comprometida por processo, quando o Windows fornece os
   contadores. Esses contadores não medem diretamente o heap memory64 do Wasm.

## Ainda não resolvido

- Arquivo de textura ausente no port não pode ser corrigido substituindo
  arbitrariamente o material por branco ou alterando sampler. Precisamos
  separar erro de *asset* (o próprio jogo registra `not found`) de
  bindings e de shaders.
- O novo caminho de mouse requer teste real de câmera, X e Y, com captura
  ativa, em janela do WGR e com hardware/movimento normal. Não rotular
  como corrigido só com unit test.
- Comparar vídeo e input com o port original autorizado, com mesma configuração
  e local de jogo. Só aceitar melhoria se a cena do personagem, câmera,
  gráficos, FPS reportado e memória de sistema/GPU forem medidos sem regressão.
- Não são declarados ganhos quantitativos em FPS, RAM ou VRAM nesta etapa.

## Verificação da entrega

- Os **12 testes** do `wgr-rage` passaram, incluindo o movimento X/Y com
  cursor capturado, substituição de recursos GPU com IDs reciclados,
  uniformes dos materiais de personagens, save e streaming de arquivos.
- `cargo build --release --locked` do `wgr-rage` passou; executável
  publicado na raiz do repositório e comparado por SHA-256 com o build:
  `07F35A53C8329BD13E69C7C754CD93B8D3EBA08DF06F9A54DF9F8640C0F9ED0A`.
- O smoke do aplicativo `WGR.exe` passou **3/3** execuções com a fixture
  Órbita, input e save restaurado, mediana de 99 ms até o primeiro frame
  reportado. **Não** é smoke visual do GTA.
- `cargo clippy --release` passou com **30 avisos existentes no Muguet**.
  O modo `-D warnings` falha por avisos prévios de estilo/refatoração:
  não foram suprimidos nem corrigidos às cegas.
- `scripts/Benchmark-Gta.ps1` passou na validação do parser PowerShell;
  o benchmark de uma sessão GTA completa **ainda não foi executado após
  estas mudanças**. Assim, ganhos de FPS/RAM/VRAM e correção de personagens
  ou câmera permanecem **não verificados na prática**.
- Antes de usar a nova versão, há backups dos executáveis em
  `artifacts/WGR.pre-gta-corrections-20261120.exe` e
  `artifacts/wgr-rage.pre-gta-corrections-20261120.exe`.
