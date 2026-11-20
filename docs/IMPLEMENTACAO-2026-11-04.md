# Retomada do player — 04/11/2026

O usuário autorizou retomar a implementação, mantendo o launcher atual, e
confirmou continuar **sem downloads de dependências**. A primeira etapa tratou
de publicação do input e samplers GPU; a continuação integrou uniformes e
constantes inline, descritos ao final.
Não conclui o player GTA nem altera as capacidades anunciadas de página/saves.

## Modo inicial e input

- `loader.rs` não passa mais `-wasmstartchoice`. O modo chega pela word em
  `input + 440`, como em `play.html`. Incluído `StartMode::SandboxEnvTest = 3`
  no mapeamento; Menu continua representando a espera por uma escolha.
- `EngineHost::start_with_launch` configura tamanho/modo antes dos construtores.
  `game_host` usa esse caminho, sem alterar os demais testes de `start(args)`.
- A publicação valida ponteiro não nulo, alinhamento e os 444 bytes do bloco
  antes de expor o endereço. Inicializa tamanho, centro, foco inativo e modo.
  Foco só será ativado por uma janela real, não por um teste headless.
- `Host::set_start_mode` retém a escolha antes da publicação e atualiza o bloco
  depois dela. A mesma trava serializa as duas operações para não perder uma
  escolha concorrente. Republicar o mesmo endereço não reinicializa teclas ou
  mouse; publicar outro endereço na mesma sessão retorna erro.
- A escrita direta de modo no helper rejeita valores fora de 0–3.

`tests/input_publication.rs` contém seis fixtures/regressões: ausência da flag,
modo pendente/atualização, publicação concorrente, geometria/modo inválidos,
ponteiros nulos/desalinhados/fora da memória ou acima de 4 GiB e ordem antes dos
construtores. Usam WASM original da fixture, sem arquivos do GTA.

As verificações da flag indevida e do ponteiro nulo também foram executadas
contra cópias dos fontes anteriores: ambas falharam como esperado. Os fontes
atuais não foram revertidos; os seis testes completos passaram separadamente.

Ainda não há janela nativa ligada a teclado, mouse, roda ou mudança de foco.
A confirmação de escolha enviada pelo jogo também não foi integrada à UI.
`wasm_has_page_js` permanece no caminho anterior; não foi ativado para simular
um serviço interativo que ainda não existe.

## Samplers

`gpu/sampler.rs` traduz as 13 words de `D3D11_SAMPLER_DESC`, segundo
`artifacts/fiveplay-wgpu.js:205–220`: filtros min/mag/mip, endereço por eixo,
comparação, anisotropia e LOD. Reutiliza a tabela de comparação já existente
em `gpu/pipeline_state.rs`. Payload curto e LOD não finito retornam erro antes
da chamada wgpu. As aproximações de border/MirrorOnce e MipLODBias do JS estão
explícitas; não foi adicionado suporte D3D11 completo.

O worker usa esse descritor em CREATE_STATE de sampler, em vez de criar um
sampler padrão e descartar o payload. A fixture de ring agora envia o descritor
completo e amostra a textura. Acrescentada comparação entre filtro point e
linear sobre texels vermelhos/azuis, com leitura dos pixels da GPU.

Contraprova em cópia temporária do worker anterior: o teste de filtro linear
falhou com `[0, 0, 255, 255]`, em vez da mistura vermelho/azul. Para chegar a
essa verificação, a cópia temporária do teste omite apenas a nova asserção de
payload curto. Nenhum fonte do projeto foi revertido para executar a contraprova.

## Validação local e limites do ambiente

`.cargo`/registry continuam ausentes. O compilador, rustfmt e Clippy 1.98.1
ainda existem em `.rustup`. Nenhuma instalação, download, mudança global de PATH,
alteração do lockfile ou substituição de executável foi feita nesta rodada.

Foi possível reutilizar os artefatos já compilados em `target/release/deps`:
um executor temporário associa as dependências aos fingerprints existentes e
chama `rustc` diretamente. A biblioteca inteira do player gerou uma nova RLIB.
Isso não equivale a linkar um executável release ou reconstruir dependências.

Para executar testes sem a import library desaparecida de winit
(`windows.0.52.0.lib`), crates temporários referenciam os **mesmos arquivos reais**
do host/GPU, sem copiar suas implementações. Excluem a surface/winit dos testes
direcionados. Não são mocks dos contratos testados; também não comprovam a
integração da janela ausente. Comandos e logs ficam em:

`C:\Users\maico\AppData\Local\Temp\opencode\wgr-startup-20261104`.

Resultados obtidos:

| Verificação | Resultado |
| --- | --- |
| `cargo fmt --all -- --check`, via toolchain e ambiente offline temporário | passou |
| Biblioteca completa via `rustc`, com dependências compiladas locais | RLIB gerada |
| `clippy-driver -D warnings` direto: biblioteca, unidades do player, fixture input e ring | passou; não substitui Clippy do workspace |
| Sampler + tabelas de pipeline, testes puros | 10 passaram |
| Unidades do host/input/loader/IO/áudio no crate direcionado | 20 passaram, 1 hardware ignorado |
| Nova integração de publicação/input | 6 passaram |
| Integrações initialization, pthread e HTTPFS | 7 passaram |
| Ring GPU com amostragem point/linear e leitura de pixels | 1 passou; sem aviso de ausência de adaptador/device |
| `game_host`, com a API atualizada | checagem de tipos passou; teste real não executado |
| Cargo Clippy/testes do workspace e build release, `--locked --offline` | bloqueados: `no matching package named anyhow found` |
| Smoke do launcher existente (`scripts/Smoke.ps1`) | bloqueado pela política de execução de scripts do PowerShell; política não alterada |

São 44 testes direcionados passando e 1 ignorado, não uma rodada completa do
workspace. Não somar esse número às rodadas históricas como testes diferentes.
O teste `game_host` foi atualizado e ainda precisa execução real posterior;
nenhum novo bootstrap GTA, escuta humana ou frame GTA foi verificado.

As duas delegações ao GPT-5.6 Luna falharam por limite de uso da assinatura;
nenhum trabalho de subagente foi incorporado. A implementação e a revisão
desta rodada foram feitas pelo orquestrador, sem mudar silenciosamente de modelo.

## Continuação

Faltam FS/saves, imports zerados, ciclo completo de pthreads, integração de
eventos/janela, storage/UAV, depth/MSAA, operações GPU assíncronas e
apresentação da textura do jogo. IO continua serial. O ambiente ainda impede
validação Cargo completa. Launcher/executáveis e referências continuam preservados.

## Continuação: uniformes e constantes inline

- `gpu/uniform.rs` valida o pack antes da alocação: até 64 KiB e até o limite
  do device, membros contíguos/alinhados em ordem de binding, sem duplicatas.
  A cópia usa `first * 16`, tamanho do membro e bytes disponíveis do shadow;
  CB ausente, final curto e offset além do EOF ficam zerados. Como no JS,
  `ConstantBuffer.size` não reduz a cópia.
- `shader::pack_uniforms` agora retorna `Result` e verifica multiplicação/soma
  em `u64`, bindings duplicados/inválidos e mistura de grupos antes de gerar
  um layout incorreto. Arrays que excedem o limite são rejeitados antes de criar
  o módulo GPU no worker.
- O binding 0 recebe buffer imutável por desenho e offset dinâmico `[0]`.
  Upload em CB somente CPU não submete o encoder. Mudanças entre desenhos,
  inclusive no mesmo recurso/offset, não alteram snapshots anteriores.
- Há limite de 16 MiB de snapshots por encoder: o worker submete antes de
  excedê-lo e retoma o render target. Esse limite cobre os bytes lógicos dos
  snapshots, não é uma medição da memória física do driver. Ring/cache e
  otimizações de alocação continuam pendentes.
- `register_shader_with_constants` reutiliza `gpu::shaders::ShaderSource`,
  aceitando diretamente o resultado de `source_from_pack`; a API antiga de
  registro só de WGSL permanece disponível. A tabela JSON numérica vira words
  com conversão equivalente a `v >>> 0`, cada escalar seguido de 12 bytes zero.
  Array vazio produz 16 bytes zerados. Limites: 64 KiB de dados, respeitando o
  device, e 256 KiB de JSON. Tipos não numéricos são rejeitados.
- O `Program` mantém o buffer inline no binding 15, sem offset dinâmico.
  Tabela ausente e tipos ainda não implementados (storage/UAV) retornam erro
  explícito, em vez de gerar bind group parcial.

`tests/uniforms.rs` usa shaders/JSON próprios offline. Cobre os dois estágios,
ordem de múltiplos membros, offsets, EOF, CB ausente, snapshots entre desenhos,
constantes inline extraídas de pack, tabela vazia/ausente, overflow/limites e
retomada após submissão por orçamento. O helper de leitura de pixels foi movido
de `ring.rs` para `tests/common/mod.rs` por referência, sem duplicar o corpo.

A fixture inicial compilou contra a RLIB anterior e falhou na validação wgpu:
o pipeline esperava bind group no índice 0. Uma primeira tentativa da fixture
tinha SET_RENDER_TARGETS curto; esse erro da fixture foi corrigido antes da
contraprova válida. Nenhum fonte atual foi revertido.

Backup desta continuação, conferido por SHA-256:
`C:\Users\maico\AppData\Local\Temp\opencode\wgr-uniforms-20261104`.
O executor e os logs `uniforms-*` continuam na pasta temporária da etapa anterior.

Validação desta continuação, sem somar execuções repetidas ou rodadas anteriores:

| Verificação | Resultado |
| --- | --- |
| Biblioteca completa via rustc e dependências locais | RLIB gerada |
| Unidades dos módulos reais de host/GPU, excluindo surface/winit | 87 passaram, 1 hardware ignorado |
| Integração de uniformes/inline/limites com GPU real | 5 passaram |
| Ring GPU, após extrair o helper compartilhado | 1 passou |
| Smoke headless `draws_a_frame_from_an_uploaded_texture` | 1 passou, com leitura de pixels; não é apenas a geração da RLIB headless |
| Input/publicação, inicialização, pthread e HTTPFS | 13 passaram |
| `cargo fmt --all -- --check` | passou |
| Clippy direto `-D warnings`: biblioteca, unidades e fixtures uniformes/ring | passou |
| Cargo Clippy/testes/build release do workspace, `--locked --offline` | tentados; bloqueados por `no matching package named anyhow found` |
| Smoke `scripts/Smoke.ps1` do launcher existente | tentado; bloqueado pela política do PowerShell, sem alteração de política |

Total: **107 testes direcionados passaram, 1 ignorado**. Isso não comprova o
workspace completo, a integração surface/winit nem um executável release novo.
Não houve downloads, alteração de manifestos/lockfile ou troca dos executáveis.

Isso ainda não liga o carregamento de shaders reais ao host nem implementa a
apresentação da textura do jogo. Nenhum novo frame GTA foi verificado.

## Retomada: ambiente, cache de pipelines e IO concorrente

O usuário autorizou recompôr o ambiente. `cargo fetch --locked` restaurou o
registry em `~/.cargo` sem alterar o lockfile; RTK 1.1.0 foi instalado por
`winget`. A partir daí, fmt, Clippy, testes e build release rodaram com Cargo
1.98.1, `--locked --offline`.

- `gpu/pipeline_cache.rs`: cache LRU limitado a 128 pipelines. A chave cobre
  geração de cada shader (não o id/hash, que pode ser reutilizado), layout,
  strides, topologia/índice, blend, depth, raster, formatos de alvo e amostras.
  Uniformes, snapshots, SRVs e samplers não invalidam o pipeline. Um acerto
  reutiliza também os bind group layouts; a eviction preserva a entrada quente.
- `gpu/worker.rs`: `pipeline::plan` e a criação wgpu só ocorrem no miss; a
  geração de shader usa `checked_add` em vez de `wrapping_add`.
- `io.rs`: o HTTPFS passou a usar um pool fixo e limitado (até 4 workers), cada
  um atendendo uma lane/bloco por vez, com orçamento agregado de temporários.
  Erro de leitura de uma lane publica `u32::MAX` e mantém o pool; corrupção do
  ring ou pânico encerra todos, com `catch_unwind`, shutdown e `join`. Testes
  usam canal/deadline, sem `sleep` para provar concorrência.
- `wgr-app/native_child.rs`: os diagnósticos de input deixaram de ser ligados
  por omissão em jogo normal. Continuam ligados em `--gta-smoke`, `--gta-logs`
  ou `WGR_INPUT_DIAGNOSTICS` diferente de `0`.
- `scripts/Validate.ps1` aceita `-SkipPublish` e `-TargetDir`, restaura o PATH
  e só limpa o build que criou. `Validate.cmd`/`Smoke.cmd` iniciam o PowerShell
  com `-ExecutionPolicy RemoteSigned` apenas no processo; nenhuma política
  global foi alterada.
- `scripts/Benchmark-Gta.ps1` passou a acompanhar também `wgr-rage.exe` e os
  descendentes do runtime, em vez de medir apenas o processo do launcher.
- `vendor/muguet/src/gpu.rs`: ao exceder o limite, o cache de bind groups
  descartava tudo, forçando recriação em massa no quadro seguinte. Agora poda
  para os mais recentes (uid monotônico), registrando o tamanho antes/depois.

Validação desta retomada:

| Verificação | Resultado |
| --- | --- |
| `cargo fmt --all -- --check` | passou |
| `cargo clippy --release --workspace --all-targets --locked -- -D warnings` | passou; permanece só o aviso de `proc-macro-error2` |
| `cargo test --release --workspace --locked --offline` | **148 passaram, 4 ignorados** |
| Cache de pipelines com GPU real (hit, invalidação, eviction) | 3 passaram |
| HTTPFS (unidades + integração) | 94 + 3 passaram |
| Build release do workspace e de `wgr-rage` | passou |
| Smoke do launcher release (`scripts/Smoke.cmd`) | 3/3, frame, input e save restaurado |
| Benchmark GTA com a poda de bind groups | executado; números no relatório da rodada |

O smoke usa a fixture Órbita, não o GTA. O número 148 inclui os testes novos e
os históricos, executados uma vez nesta rodada. O benchmark mede o runtime
nativo com o cache autorizado; ele não transforma o player novo em jogável e
não prova correção visual dos personagens.

## Diagnóstico visual e mouse — continuação

O probe de `ped::PS_DeferredPedTexturedC` agora registra recurso, escritas e
bytes iniciais. No run `probe-ped-writes-1`, o corpo usou `srv=25533`, recurso
`25532`, BC1 `4x4`, `writes=1` e bytes `ff7b1f84aaaaaaaa`. O bloco BC1 decodifica
para aproximadamente `(127,127,255)`, portanto não é uma textura ausente nem
um upload zerado. A causa dos personagens pretos continua aberta; não foi
aplicado fallback específico.

O mesmo run também mostrou input horizontal chegando ao engine: `raw_x_events`
e `relative_x_units` cresceram, e o heading/câmera mudaram. O snapshot passou a
expor `pending_dx` e `pending_dy` para separar perda de evento, consumo pelo
jogo e efeito visual. O diagnóstico de texturas pequenas só é ativado com
`MUGUET_TEXTURE_DIAGNOSTICS`, sem alterar o caminho normal.
