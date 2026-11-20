# Auditoria local — 03/11/2026

Retomada de implementação em 04/11, sem downloads: publicação/modo de input e
samplers corrigidos. A validação e os limites dessa rodada estão em
[IMPLEMENTACAO-2026-11-04.md](IMPLEMENTACAO-2026-11-04.md).
O histórico abaixo mantém o estado observado durante a auditoria de 03/11.

## Escopo e método

Revisão dirigida de segurança, importação/ZIP remoto, cache, HTTP, catálogo,
persistência, host nativo, memória/ring e backend GPU. O Graphify local examinou
68 arquivos de código: 1.479 nós e 3.088 relações AST, sem extração por LLM.
As relações foram conferidas no código; o mapa não comprova correção semântica.

Aplicadas Caveman, Ponytail, Graphify, RTK e Zero-Write. Relidos a Zero-Write
2.1.0 dos Downloads e seus contratos de CLI/composição; alterações anteriores
de TLS/pthreads preservadas. Referências MomorRust e 5-web-port somente lidas.

## Falhas corrigidas

| Área | Falha | Correção / cobertura |
| --- | --- | --- |
| `wgr-core/security.rs` | Normalização aceitava `a/./b`, `a//b` e barra final; nomes Windows ambíguos passavam | Validação dos segmentos brutos, controles, caracteres proibidos e dispositivos; casos válidos preservados |
| `wgr-core/assets.rs` | Hash corrompido no SQLite causava panic sob mutex ou construía caminho inválido | SHA-256 canônico validado antes de fatiar; erros propagados em lookup, promoção, eviction e limpeza |
| `wgr-core/gta.rs` | Nome ZIP truncado causava panic; membro Stored podia declarar tamanhos divergentes | Comprimento mínimo e acesso verificado ao nome; igualdade dos tamanhos Stored; índice vazio rejeitado |
| `wgr-http` | `If-Range` ignorado; HEAD retornava 206; Range suprimia precondição de cache | Range somente em GET, com validador forte compatível; `If-None-Match` precede Range e aceita lista, wildcard e comparação fraca |
| `gpu/worker.rs` | Payloads truncados e contagens absurdas causavam panic/alocação; opcodes ausentes retornavam sucesso | Validação antes de indexar/alocar; estágio e slots limitados; operações ausentes retornam erro explícito |
| `gpu/state.rs`, `worker.rs` | Stride/offset invertidos, formato do índice perdido, strides ausentes no pipeline e opcode 38 ignorado | ABI corrigida; desenho indexado com offset e stride não zero verificado por pixels; estado UAV roteado |
| `gpu/upload.rs`, `worker.rs` | Padding da última linha exigido indevidamente; uploads podiam exceder recurso ou ser truncados silenciosamente | Cálculo verificado de bytes, alinhamento e limites; erros sem truncar o buffer |
| `gpu/worker.rs` | Upload posterior alterava textura de desenho já gravado | Submissão do encoder anterior antes de `Queue::write_*`; pixels vermelho/azul comprovam a ordem |
| `gpu/worker.rs` | Clear sem draw era perdido ou atingia o slot zero; troca de alvo mantinha pass antigo | Clear gravado no recurso/mip/slice solicitado, mesmo desligado; troca de alvos encerra o pass |
| `gpu/pipeline.rs` | Alpha-to-coverage ativado sem MSAA e desativado com MSAA | Condição corrigida; teste puro com uma e quatro amostras |
| `gpu/table.rs` | Um ID esparso grande podia alocar bilhões de slots | Mapa esparso com iteração ordenada; teste de inserção, alteração e remoção de `u32::MAX` sem alocação proporcional ao ID |

As regressões de caminhos/cache, ZIP/HTTP, payload GPU, padding, clear,
ordenação de upload e alpha-to-coverage falharam antes das respectivas correções.
O caso de ID extremo não foi executado na versão antiga para evitar OOM.

## Validação da rodada anterior a IO/áudio

- `cargo fmt --all -- --check`, Clippy release do workspace com `-D warnings`
  e build release com `--locked`: passaram.
- Workspace: 102 testes passaram, 3 ignorados. Ring GPU reexecutado com saída
  visível: passou, sem mensagem de ausência de adaptador/device.
- Superfície real (`surface_frame`, ignorado por padrão): passou.
- Smoke Órbita com `target/release/WGR.exe`: 3/3, input Win32 e save restaurado.
  Primeiro frame entre 90 e 104 ms; mediana de 104 ms. Não é frame do GTA nem
  medição física de apresentação. Gamepad físico e escuta humana não testados.
- Fixtures Node `ram-io.mjs`, `session-observer.mjs` e `gpu-observer.mjs`: passaram.

## Continuação: IO/áudio, bootstrap e contratos

- Implementados `wgr-player/src/io.rs` e `audio.rs`: manifesto/lanes HTTPFS,
  arquivos locais/remotos autorizados e ring/saída CPAL. Corrigidos também
  limite de heap anunciado, default de opções do módulo e consumo de WRAP.
- Manifesto real: 5814 entradas `[nome,tamanho,mtime]`, 393014 bytes, versão
  `1791169379-5814`. O parser passou a aceitar o terceiro campo; fixtures de
  dois campos continuam aceitas. Transporte codifica caminhos e versiona URL.
- Validação posterior do player: 78 testes de biblioteca passaram, 1 ignorado;
  integrações initialization/pthread/HTTPFS: 7 passaram. Teste explícito de
  dispositivo CPAL real e mudo passou; não houve escuta humana.
- `Cargo.lock` acrescentou nove pacotes para CPAL e dependências; nenhum pacote
  anterior foi removido ou atualizado nessa resolução.
- Bootstrap com hash WASM conferido passou dos construtores e de
  `MountAudioPacks`, chegando à pré-carga de shaders. Watchdog interrompeu após
  20 s em `wasm_httpfs_pread`/`grcEffect::Preload`, com
  `gpu_started=true io_started=true audio_started=true`. Sem frame do GTA.
- Leitura do port distinguiu página/servidor, glue, loader, workers originais e
  adaptações específicas do WebView WGR. Há 57 imports no fallback zero,
  modo inicial enviado pela via errada e grandes lacunas de bindings/GPU/FS.
  Essas descobertas foram documentadas, não corrigidas nesta fase.
- Contrato `node tests/ram-worker-contract.mjs artifacts/io_worker.js` passou,
  inclusive na análise posterior: original 262144 bytes, adaptado 4096 bytes,
  sem OPFS. A fixture existe; a afirmação anterior de ausência estava errada.

Detalhes e ordem de implementação:
[CONTRATOS-5-WEB-PORT.md](CONTRATOS-5-WEB-PORT.md).

Depois do desaparecimento de `.cargo`, o usuário decidiu **somente analisar**.
Não houve reinstalação nem alterações Rust nesta fase. A rodada completa de
fmt/Clippy/testes/build/smoke acima antecede IO/áudio e não valida o estado atual
do workspace. Números das rodadas não devem ser somados.

## Limites e pendências

- Esta revisão não é prova de ausência de todos os bugs: não houve fuzzing
  exaustivo, auditoria formal de `unsafe`, teste de corrida nem revisão de todas
  as dependências e referências externas.
- O novo player atende HTTPFS básico e possui saída CPAL, mas não apresenta
  frame real do GTA. IO é serial e sem cache/hints/prefetch/batches; o worklet
  original ainda não foi localizado para comparação integral. O log real
  contém “Headless mixer”; `audio_started=true` não prova áudio útil do jogo.
  `gpu_started=true` também só registra publicação; `wgr-rage` segue padrão.
- GPU ainda sem paridade completa: bindings de uniformes/storage, estados e views
  avançados, profundidade/estêncil, scissor, compute, cópias, queries e readback do
  protocolo precisam implementação/validação. Contadores iniciais UAV diferentes
  de `u32::MAX` e draws indiretos agora falham explicitamente. PRESENT no dreno
  submete comandos; não comprova apresentação do jogo em janela.
- A submissão conservadora antes dos uploads prioriza ordem correta; seu custo
  no jogo real não foi medido. A tabela esparsa troca acesso por índice por busca
  ordenada, evitando consumo proporcional ao maior ID.
- Testes de GPU podem retornar sem device em máquinas sem adaptador. O resultado
  local precisa ser conferido com execução explícita e saída visível.
- Permanece o aviso futuro E0365 de `proc-macro-error2 2.0.1`: reexport privado
  de `proc_macro`, transitivo de `gpui → stacksafe → stacksafe-macro`. Não é
  falha atual do Clippy; não foram alterados registry, lockfile ou dependências
  de referências somente leitura para escondê-lo.
- `engine.rs` ainda retorna zeros para imports ausentes, inclusive FS/saves,
  relógios, mailbox e capacidades. `loader.rs` ainda passa `-wasmstartchoice=2`,
  embora a página original use o bloco de input; seu teste atual perpetua a
  divergência. Mudar apenas a flag de presença de página não completa o host.
- Cargo/RTK no PATH e dependências continuam indisponíveis após o
  desaparecimento de `C:\Users\maico\.cargo`. Não é possível afirmar nova
  validação Rust/release/smoke; a implementação está pausada por decisão do usuário.
- Não foram alterados launcher, executáveis da raiz, flags de segurança nem
  referências. Sem commit/push.
