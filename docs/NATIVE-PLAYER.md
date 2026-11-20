# Player nativo — reescrita Rust baseada no 5-web-port

## Integração no WGR.exe (20/11/2026)

O launcher `wgr-app` depende agora do crate `wgr-player` **diretamente em
Rust**, para o diagnóstico GPU. A tela Diagnóstico disponibiliza
**Testar GPU nativa (Rust / wgpu)**; o comando `WGR.exe --native-check` executa
o mesmo teste sem abrir a interface e grava `native-check.json` na pasta de
dados. O teste cria um dispositivo wgpu, renderiza uma textura vermelha de 1x1,
copia a textura de volta à memória e verifica os quatro canais RGBA. Isso
verifica o dispositivo e a fila de comandos reais, mas não prova GTA.

**A apresentação do GTA no WGR continua com `wgr-rage.exe`**, que já executa
Wasmtime/wgpu/CPAL nativamente. O crate `wgr-player` ainda não substitui esse
adaptador: seus imports e GPU ring incompletos não permitem prometer um jogo
completo. Referências Servo, Surfman, WebRender e MozJS ajudam a estudar
superfícies, isolamento, compilação Wasm e gráficos, mas **nenhum desses
motores entra no binário principal**. Não há HTML/DOM/JS ou WebView no caminho
padrão.

O `assets/gta/zip_index.json` de origem não é mais obrigatório no build. O
aplicativo conserva o índice já configurado no perfil e pode copiar um índice
externo colocado ao lado do EXE, em `assets/gta/zip_index.json`, quando ele
existir. A origem ZIP antiga requer um índice válido e autorização de assets;
o launcher não inventa dados nem ignora validações.

O objetivo do player nativo é substituir o WebView2 executando a mesma engine
WASM em Wasmtime, com GPU wgpu, entrada nativa, áudio CPAL e leitura de assets
sob demanda. A integração ainda está incompleta, sem frame GTA verificado.
A referência funcional é o 5-web-port (`public/index.html`,
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
| `play.html` (input/tamanho) | `input.rs`, `engine.rs` | publicação/modo testados; eventos/janela ainda não integrados |
| `wgpu_worker.js` | `shader.rs`, `gpu/*`, `drain.rs`, `surface.rs` | backend parcial; `PRESENT` já compõe a textura quando recebe uma superfície; integração ao ciclo da janela e operações restantes pendentes |
| `loader.js`, `game.js` | `engine.rs`, `wgr-runtime` | bootstrap/TLS/pthreads; imports, FS e ciclo de vida ainda parciais |
| `io_worker.js` | `io.rs` | lanes HTTPFS locais/remotas testadas; serial, sem cache/hints/bootset |
| `play.html`, import de áudio | `audio.rs` | ring/CPAL testados; worklet original não localizado, áudio útil do jogo não verificado |
| `loader.js`, userdata do IO | pendente no host | restauração/FS/saves do novo player ausentes |
| `server.js` | `wgr-http`, `wgr-core` | transporte existente; não equivale à paridade de todos os endpoints/otimizações |

## Contratos já fixados

- Bloco de input: 256 bytes de teclas, dez palavras i32 (`mouseX`, `mouseY`,
  `dX`, `dY`, `dZ`, botões, foco, capturado, cabeça e cauda), 64 u16 de texto,
  extensão (`winW`, `winH`) e `startMode`. O host escreve com operações
  atômicas; a engine lê direto da memória compartilhada.
- Mouse capturado: `dX/dY` acumulam o movimento relativo e `mouseX/mouseY`
  acompanham com clamp no tamanho do buffer. Sem captura, a posição absoluta é
  convertida em delta, como no `mousemove` do play.html.
- Uniformes: todos os `var<uniform>` de um grupo viram um `cbpack_t` no
  binding 0, com offset dinâmico zero e snapshot imutável por desenho;
  o binding 15 usa a tabela inline do pack. Limite de 64 KiB por buffer,
  respeitando também o device; orçamento de snapshots de 16 MiB por submissão.
- Argumentos: `-forceResolution`, `-width/-height` iguais ao buffer interno,
  `-fastscriptcore`, `-frameLimit`, `-numCores` apenas com dois núcleos ou
  menos, e o perfil de pouca memória do play.html quando solicitado.
- Tela original: tamanho da janela × DPI × escala, com limites de resolução e
  resize no bloco de input. O helper de letterbox nativo é uma escolha adicional,
  não reprodução literal do CSS original; ainda falta ligar a apresentação.

## Próximas etapas

1. Ligar o input já publicado aos eventos da janela e distinguir capacidades
   interativas de headless, sem anunciar FS/saves antes de implementá-los.
2. Completar os imports usados, FS/persistência e vida útil das pthreads.
3. Conectar o carregamento dos packs reais ao host, completar storage/UAV,
   depth/MSAA e estado dinâmico. Uniformes, constantes inline e samplers já
   estão ligados ao worker e testados por pixels em fixtures offline.
4. Conectar o `Presenter` já testado ao ciclo real da janela; completar
   fence/queries/readback, cópias/resolve, compute/UAV e mips.
5. Evoluir IO serial para concorrência/cache limitados e prioridade de demanda;
   depois hints/prefetch. Conferir áudio útil e shutdown completo.
6. Criar/integrar executável e validar frame, input e saves com bundle autorizado.
   Só então avaliar cutover; launcher atual permanece preservado.

Contratos, referências de arquivo/linha e critérios por etapa:
[CONTRATOS-5-WEB-PORT.md](CONTRATOS-5-WEB-PORT.md).

## Regras

- Nada de WebView, HTML, JavaScript ou iframe no caminho padrão.
- Flags que desativam a segurança do WebView2 não são usadas (o WebView nem é
  carregado).
- GTA só é chamado de jogável com frame, input e bundle autorizado verificados.
- Testes determinísticos usam fixtures originais e memória simulada; o smoke
  real exige GPU e desktop interativo.

## Histórico: correção sobre a base `ba8fdfb` — 03/11/2026

As alterações não commitadas da tentativa anterior foram preservadas em backup
antes da restauração. Naquela etapa ainda não havia IO/áudio nem novo executável;
a continuação abaixo registra o estado posterior.

- `_emscripten_tls_init` retorna `i64` nesta build. O host tentava encontrá-lo
  como `() -> ()` e ignorava o erro, deixando o TLS sem inicialização.
- Os exports de inicialização agora usam a assinatura declarada para alocar os
  resultados. Exports ausentes continuam opcionais; assinaturas incompatíveis
  ou traps em exports existentes são propagados.
- `_emscripten_thread_init` recebe um ponteiro `i64` e cinco valores `i32`.
  O host inicializa a thread principal como runtime principal com bloqueio
  permitido e inicializa o TLS de cada pthread antes da rotina da tabela.
- A rotina pthread pode retornar `i64`; o host não exige mais retorno vazio.
  Falhas na configuração da pilha também não são mais descartadas.

As fixtures offline `tests/initialization.rs` e `tests/pthread_fixture.rs`
cobrem essas assinaturas e a ordem de inicialização. O teste manual `game_host`
verifica somente o bootstrap: passou dos construtores e de `sysMemStartDebug`
até `gpu_started=true`, mas ainda termina em `CFileMgr::MountAudioPacks` porque
o manifesto HTTPFS não é atendido nesta base (`io_started=false`). Esse resultado
não comprova frame do jogo, áudio ou jogabilidade. O launcher continua usando
`wgr-rage`; não houve cutover.

Validação local: `fmt`, Clippy release com `-D warnings`, 98 testes do workspace
(3 ignorados) e build release passaram. A superfície passou no teste manual;
o smoke Órbita passou nas três execuções, com input e restauração de save.
Permanece o aviso de compatibilidade futura da dependência `proc-macro-error2`.

Auditoria posterior de cache, segurança, HTTP/ZIP e GPU, com regressões e limites:
[AUDITORIA-2026-11-03.md](AUDITORIA-2026-11-03.md).

## Continuação: IO/áudio e análise dos contratos

- `io.rs`: manifesto real `[nome,tamanho,mtime]`, lanes memory64, leituras
  locais/remotas autorizadas, URL codificada/versionada, cópias até 1 MiB,
  EOF/erro e encerramento. Ainda sem cache, hints ou concorrência entre lanes.
- `audio.rs`: ring estéreo 48 kHz, resampling, wrap, mute, PCM e saída CPAL.
  Biblioteca do player: 78 testes passaram, 1 ignorado; integrações de
  inicialização/pthread/HTTPFS: 7 passaram. Saída física muda passou à parte,
  sem escuta humana. Não somar esses números à rodada anterior do workspace.
- Bootstrap autorizado posterior passou de `MountAudioPacks` e alcançou a
  pré-carga de shaders. Watchdog de 20 s interrompeu em HTTPFS/`grcEffect::Preload`,
  com GPU/IO/áudio publicados. Isso ainda não é frame ou áudio útil do GTA.
- Análise estática: 85 imports de função, 17 braços explícitos, 11 `invoke_*`
  e 57 no fallback zero. Inclui FS/saves/mailbox e `wasm_has_page_js`.
  O log mostrou “Headless mixer”; a seleção precisa validação própria.
- A análise de 03/11 encontrou o teste exigindo `-wasmstartchoice=2`, rejeitado
  pela engine. A correção entrou na retomada de 04/11, descrita abaixo.
- O novo crate segue sem executável integrado; o app não depende de
  `wgr-player`, e o launcher continua usando `wgr-rage`.

Após o desaparecimento de `C:\Users\maico\.cargo`, o usuário escolheu somente
análise. RTK/Cargo no PATH e fontes de dependências ficaram indisponíveis;
nenhuma recomposição foi feita. Não houve validação completa do workspace após
IO/áudio. Nesta fase só foram atualizados os documentos e repetido o contrato
offline Node do worker IO, que passou. GTA permanece **não verificado como
jogável** neste novo player.

## Retomada de 04/11/2026

O usuário liberou implementação, sem recompor dependências por download.
Corrigidos modo inicial/publicação de input e interpretação dos samplers.
Na continuação, integrados uniformes por desenho e constantes inline dos packs,
sem reutilizar buffers mutáveis entre desenhos pendentes.
Fixtures direcionadas passaram usando rustc e artefatos locais; a validação
Cargo completa continua bloqueada pelo registry ausente. Não houve troca do
launcher nem novo frame GTA.

Alterações, testes executados e limites:
[IMPLEMENTACAO-2026-11-04.md](IMPLEMENTACAO-2026-11-04.md).
