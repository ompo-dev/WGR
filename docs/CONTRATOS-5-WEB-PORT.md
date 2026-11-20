# Contratos do 5-web-port e equivalentes Rust

Atualização de 04/11: modo/publicação de input, descritores de sampler,
uniformes por desenho e tabelas inline foram implementados após esta análise.
Estado e validação em
[IMPLEMENTACAO-2026-11-04.md](IMPLEMENTACAO-2026-11-04.md).
As referências de linha abaixo descrevem o snapshot analisado em 03/11.

Análise local de 03/11/2026, build `8b0b5899ed`. **Documento de implementação,
não declaração de paridade.** O usuário autorizou o cache atual e, depois do
desaparecimento de `.cargo`, escolheu continuar somente a análise. Nesta fase
não houve alteração Rust, reinstalação, download de bundle nem troca do launcher.

## 1. Evidências e limites

Página, servidor, `app.js`, README, loader e worker IO foram lidos integralmente.
No worker GPU foram examinados os contratos de inicialização, recursos, shaders,
bindings, draw/compute, cópias, queries, readback, apresentação e despacho.
O glue minificado foi consultado por funções; não foi reexecutado para esta análise.

As referências abaixo são relativas à raiz WGR; linhas correspondem ao snapshot
local. O repositório `../5-web-port` contém página/servidor, mas os workers e o
glue vêm do bundle, não do código daquele repositório.

| Snapshot local | Bytes | SHA-256 |
| --- | ---: | --- |
| `artifacts/fiveplay-loader.js` | 9201 | `9de25898fa9a9ec5e67ca647debf941849b936e3163402944581feaaae5ac0ff` |
| `artifacts/fiveplay-io-worker.js` | 41287 | `d91c89e64f7f280666b5c9d1669ed6d65d66605077264569a49146ed79fe0bae` |
| `artifacts/fiveplay-wgpu.js` | 194031 | `826dfaa9012dee081db6985b8973c18882ccafae23f935f6eb18eec392ebec58` |
| `artifacts/fiveplay-game.js` | 124240 | `81a7d6e912238de3d89bcafb1b01450671c6793167babb0ec970f308f454bcf0` |

`artifacts/io_worker.js` existe e tem o mesmo hash do worker IO acima.
`audio-worklet.js` não foi localizado no repositório de referência, nos artifacts
nem por nome/referência no cache local consultado. Seu algoritmo não foi comparado
integralmente; a página e os imports só comprovam parte do contrato de áudio.

O grafo Graphify anterior (1.479 nós, 3.088 relações) orientou a busca, mas antecede
IO/áudio; todas as conclusões abaixo foram conferidas nos fontes atuais.

## 2. O WebView não é a engine

```text
janela / página (DOM, menu, teclado, mouse, áudio, progresso)
  └─ loader worker (runtime principal, game.js, memória compartilhada)
       ├─ instância WASM principal → main(), pode bloquear
       ├─ pthread workers → outras instâncias do mesmo módulo/memória
       ├─ GPU worker → ring → WebGPU → canvas transferido
       └─ IO worker → lanes/hints → HTTP/cache e IndexedDB de saves

server.js → arquivos locais OU membros ZIP remoto → Range/batch
```

WebView2 fornece Chromium: JavaScript/WebAssembly, workers, WebGPU,
SharedArrayBuffer/Atomics, AudioWorklet, eventos DOM e storage. Uma substituição
nativa precisa oferecer esses serviços e sua sincronização; não precisa
implementar navegador, interpretar HTML nem executar os scripts originais.
Ela continua executando a engine WASM via Wasmtime: não é recompilação da engine
C++ para código Rust.

No WGR, o caminho WebView também tem adaptações próprias:

- `crates/wgr-app/src/webview.rs:552–559,614–622,648–730`: configura o host,
  injeta observação, confere origem das mensagens e bloqueia navegação/permissões.
- `webview.rs:841–892,1056–1094`: deixa recursos locais/HTTP nativo seguirem seus
  caminhos apropriados; intercepta especificamente WASM/loader/IO no caso remoto.
- `session_observer.js:4–35`: compila o módulo antes de transferi-lo ao loader;
  também aplica o perfil balanced (`textureQuality=1`, `lodScale=0.5`), salvo
  argumentos manuais. Isso é comportamento WGR, não default da página original.
- `gta_browser.rs:5–93`: exige hashes conhecidos, cria workers via Blob,
  injeta diagnóstico GPU e acrescenta `wgr-http/src/ram_io.js` ao IO original.
- `ram_io.js:286–375`: a adaptação persistente desativa o OPFS original e usa
  ranges em IndexedDB, RAM limitada, demanda prioritária e prefetch limitado.

Portanto, **IO original e IO adaptado do WebView não são idênticos**. Reutilizar
seu protocolo não significa copiar todas as decisões de cache de ambos.

## 3. Ordem real de inicialização

1. A página exige isolamento de origem, transfere o canvas e remove caches
   antigos de service worker (`play.html:370–408`). Calcula resolução e perfil.
2. Cria o loader e envia argumentos, canvas e opções (`play.html:420–447`).
3. O loader cria GPU/IO **antes** de carregar o glue e bloquear em `main`.
   Inicia manifesto/cache/prefetch IO em paralelo e espera os dois sinais
   `loaded` (`fiveplay-loader.js:11–29`). `loaded` não significa device GPU pronto.
4. Configura `Module`, instancia/compila WASM e compartilha o módulo com pthreads
   (`fiveplay-loader.js:42–104`; funções `createWasm` e `PThread` no glue).
5. `preRun` restaura `/userdata/` em MEMFS, com barreira e timeout de 8 s
   (`fiveplay-loader.js:105–139`). Isso ocorre antes dos construtores/main.
6. O glue inicializa FS/TTY, construtores e runtime; chama `__main_argc_argv`
   com `argc: i32`, `argv: i64`, ponteiros de 8 bytes e terminador nulo.
7. Imports publicados pela engine entregam os endereços de input, áudio e IO.
   `wgpu_start_worker(ring,size,dbg)` envia memória/canvas/shaderUrl ao GPU.
8. Somente após criar o device, o consumidor GPU escreve estado running no ring
   e drena comandos (`fiveplay-wgpu.js:2900–2958`).

Em Rust, janela/eventos e serviços precisam continuar ativos enquanto `start()`
bloqueia numa thread da engine. O host ainda só registra ring/input em
`engine.rs:341–345,374–380`; não inicia ali uma sessão completa com janela/GPU.
Não substituir `loaded`, publicação de ponteiro, device pronto e primeiro frame
por um único booleano de sucesso.

## 4. Memória, ABI e pthreads

- Glue `initMemory`: memória **memory64 compartilhada**, inicial de 3 GiB,
  máximo declarado de 16 GiB. O orçamento nativo pode limitar o máximo e deve
  anunciar esse limite honestamente em `emscripten_get_heap_max`.
- Endereços são `i64`; campos low/high dos rings recompõem `u64`, sem truncar
  endereços acima de 2/4 GiB. Crescimento invalida views JS, não o protocolo.
- Metadados de conclusão precisam ser publicados depois do payload: bytes →
  resultado → flag/tail → notify. Reutilizar ring antes de copiar o payload
  assíncrono corrompe dados.
- Glue `establishStackSpace` lê high/size em `pthread + 80/+88`; depois executa
  thread init, TLS, mailbox e rotina da tabela. O Rust já trata pilha/TLS/ABI,
  mas `run_pthread` termina após `call_table` (`engine.rs:145–200`).
- No glue, `invokeEntryPoint` chama `_emscripten_thread_exit` quando a rotina
  retorna e o runtime não deve permanecer vivo. O Rust descarta esse resultado
  (`engine.rs:127–141`), não reproduz essa saída, não guarda os handles das
  pthreads e não as junta ao encerrar (`203–223,681–705`). Falta testar join,
  cleanup e encerramento, não apenas a execução da rotina.
- `invoke_*` no JS salva a pilha, distingue exceções Emscripten, restaura a pilha
  e usa `setThrew`. O Rust só chama a tabela (`engine.rs:225–239`); o caminho
  comum testado não comprova paridade de exceções/longjmp.
- `wasm_now_ms` no glue usa `performance.timeOrigin + performance.now()`;
  `emscripten_get_now` é outro relógio. O Rust os trata como tempo desde `boot`
  (`engine.rs:270–272`). É divergência de origem temporal; impacto ainda não
  reproduzido, não um bug corrigido.
- `wasm_module_int_js` no glue consulta opções de `Module` e, em Node, variáveis
  de ambiente antes do default. O braço Rust (`engine.rs:289`) sempre devolve
  o default; ter um braço explícito não significa suportar todas as opções.

Não usar implementações WASI wasm32 diretamente: neste bundle os iovecs têm
16 bytes e contagens/ponteiros de saída podem ter 8 bytes. `__syscall_*` retorna
errno negativo; os wrappers WASI, errno positivo e resultados na memória.

### Inventário estático dos imports

`artifacts/native-gta-contract.json` lista 86 imports: memória + 85 funções.
Comparação com `engine.rs::dispatch`: **17 funções têm braço explícito,
11 são `invoke_*`, 57 caem no fallback zero/no-op** (`engine.rs:458–462`). Contagem
de caminhos do dispatcher, não porcentagem de compatibilidade ou chamadas reais.

| Grupo sem tratamento específico | Exemplos e efeito a reproduzir |
| --- | --- |
| Capacidades e saves | `wasm_has_page_js`, `wasm_userdata_page_js`, `wasm_userdata_put_js`, `wasm_userdata_delete_js` |
| FS POSIX/Emscripten | `__syscall_openat`, `stat64/fstat64/lstat64/newfstatat`, `getdents64`, `getcwd`, `mkdirat`, `unlinkat`, `rmdir`, `renameat`, `ftruncate64`, `fcntl64`, `ioctl`, `poll`, `statfs64` |
| FS/ambiente WASI | `fd_read`, `fd_pread`, `fd_pwrite`, `fd_seek`, `fd_close`, `environ_get`, `environ_sizes_get`; sucesso exige preencher os destinos |
| Tempo/calendário | `clock_time_get`, `_tzset_js`, `_gmtime_js`, `_localtime_js`, `_mktime_js` |
| Threads/agendamento | mailbox await/notify, receive-on-main-thread, cleanup, strongref, blocking check, exit-with-live-runtime |
| Runtime/diagnóstico | `exit`, `proc_exit`, `_abort_js`, `_emscripten_throw_longjmp`, stack snapshot/unwind, dois imports `emscripten_asm_const_int*`, `_emscripten_system` |
| Sockets | accept4, connect, getsockname, getsockopt, recvfrom, sendto, setsockopt, shutdown, socket |

O fallback zera resultados ou ignora chamadas void. Alguns no-ops são legítimos
no glue, como blocking check e strongref. Não tratar
todos os 57 como bugs, nem presumir que todos são dispensáveis. Com `-nonetwork`,
sockets podem ser recusados com erro coerente, nunca fingir conexão por retorno 0.
O braço existente `fd_write` só imprime em stderr (`engine.rs:302–330`); ainda
não equivale à escrita em arquivos da FS original.

`ASM_CONSTS` contém ações concretas: idioma (`7980558`), abrir URL (`7980673`) e
recarregar (`7980768`). O equivalente nativo deve escrever o idioma no buffer e
tratar ações externas pela política do host, não executar JavaScript arbitrário.

## 5. Input, modo e tamanho de tela

Contrato de `play.html:488–624`, relativo ao ponteiro publicado:

| Offset em bytes | Conteúdo |
| ---: | --- |
| 0–255 | teclas VK; pressionada = `0x80` |
| 256–295 | dez i32: MX, MY, DX, DY, DZ, botões, foco, captura, textHead, textTail |
| 296–423 | ring de 64 unidades UTF-16; gravar texto antes de avançar head |
| 424/428 | limite/contagem de draws de diagnóstico |
| 432/436 | largura/altura do buffer interno |
| 440 | startMode: 0 espera, 1 história, 2 sandbox GTA V, 3 sandbox env_test |

A página retém a escolha mesmo antes de receber o bloco; escreve-a quando
instala input (`525–551`) e permite mudar até a confirmação da engine
(`211–268,297–298`). **Não envia `-wasmstartchoice`**. O Rust ainda envia essa
flag (`loader.rs:165–167`), e seu teste atual exige a flag errada (`185–201`).
O log real confirmou `Unknown option 'wasmstartchoice=2'`. Existe
`InputBlock::set_start_mode`, mas a publicação no host apenas guarda o ponteiro.

O adaptador de eventos nativo ainda precisa ligar VK/numpad, texto, aliases de
voo I/K/J/L/U/O, foco com soltura de todas as teclas/botões, captura relativa,
botões na ordem esperada e roda com frações acumuladas. Testar input no bloco
isolado não comprova eventos chegando ao jogo.

`wantSize` usa **janela × DPI × escala**, arredonda para pares e limita a
3840×2160 (2560×1440 no perfil baixo), salvo resolução forçada. Resize publica
432/436 após debounce de 150 ms (`play.html:397–407,536–546`). O canvas ocupa a
janela por CSS; o blit GPU acompanha o tamanho da textura de back buffer.
Letterbox de `input.rs::content_viewport` é uma escolha nativa adicional, não
prova de reprodução literal do CSS original; mouse e apresentação devem adotar
o mesmo viewport se essa escolha for mantida.

## 6. HTTPFS, cache e servidor

O manifesto real local tem 393014 bytes, 5814 entradas, versão
`1791169379-5814`. Cada entrada é `[nome, tamanho, mtime]`; a ordem define o ID.
O worker JS usa os dois primeiros campos; `io.rs` já aceita os três e fixtures
de dois. `wasm_httpfs_manifest_js(buf,cap)` retorna o comprimento, copia somente
se `cap > comprimento`, acrescenta NUL e retorna -1 se indisponível.

Protocolo de `fiveplay-io-worker.js:15–20,474–584`, relativo à tabela:

- Header de 64 bytes: doorbell em +0; contador de lanes em +4; offset **em
  words**, capacidade, head/tail de hints em +8/+12/+16/+20.
- Lane `i` em `table + 64 + 64*i`: state +0, fileID +4, offset low/high +8/+12,
  destino low/high +16/+20, comprimento +24, resultado i32 +28.
- Consumidor reivindica `1 → 3`, escreve bytes e resultado, publica state 2 e
  notifica. EOF = 0/contagem curta; erro = -1. Ler doorbell **antes** de escanear
  evita perder um pedido concorrente antes do wait.
- Hint: quatro words; comprimento publicado por último, bit 30 indica baixa
  prioridade. Zerar comprimento após consumo e atualizar tail; lidar com wrap e
  produtor que ultrapassou a capacidade.

O original atende lanes concorrentemente (`serve(slot)` sem await no scan).
Usa OPFS com blocos de 4 KiB, fallback LRU de 48 MiB/blocos de 256 KiB,
deduplicação em voo, bootset, read-ahead, batches e hints com prioridades.
Demanda não deve esperar prefetch inteiro (`329–403,474–568`). O journal
persiste dados **antes** de referências (`303–319`) e é vinculado à versão.

O `io.rs` atual preserva lanes/memory64, valida caminhos, lê local ou remoto,
codifica segmentos, versiona URLs e limita cada cópia a 1 MiB. `pump()` continua
serial, mas `spawn()` usa um pool fixo de até 4 workers, cada um com uma lane e
um bloco de 1 MiB por vez (orçamento agregado de 4 MiB em voo). Ainda faltam
cache, hints, bootset, batches e deduplicação. O fato de depender de `wgr-core`
não liga automaticamente seu cache de disco.
O original também ajusta o EOF quando o servidor tem arquivo menor que o
manifesto (`98–114,435–466`); Rust exige tamanho exato e retorna erro em leitura
truncada. Definir essa política explicitamente, sem armazenar bytes truncados
como conteúdo íntegro. A versão numérica é aceita pelo JS; o transporte Rust
atual só extrai versão string (`io.rs:103–108`).

`../5-web-port/server.js` é o transporte, não executor da engine: resolve
arquivos do mirror ou ZIP remoto, atende 200/206/416 e fornece COOP/COEP/CORP.
`app.js:27–71` inicia esse servidor. `server.js:750–847` implementa batches:
lista `[caminho,início,fim inclusivo]`, resposta concatenada em ordem,
`X-Run-Lengths`, gzip opcional e GET de batch pré-comprimido identificado por
SHA-1 de `versão + ':' + JSON`. No GET estático os bytes já são gzip, sem
`Content-Encoding`; no POST comprimido o header permite descompressão HTTP.
São otimizações de transporte substituíveis em Rust; os bytes e resultados
visíveis à engine devem permanecer equivalentes. Não copiar limites/validações
fracas do servidor de referência para o host nativo.

## 7. Saves e áudio são serviços separados

### Persistência

Loader `105–139`, IO `608–641` e funções `wasm_userdata_*` do glue:

- FS em memória é restaurada antes do jogo; saves/settings em `/userdata/` têm
  conteúdo **e mtime**, usado na seleção do save mais recente.
- Alterações e exclusões são enviadas ao IO e persistidas em
  `gta5-userdata/files` (IndexedDB). Não são blocos do cache de assets OPFS.
- Nativo precisa de FS compatível e perfil persistente próprio, caminhos
  confinados, escrita segura e flush/encerramento; migração de saves do browser
  seria trabalho separado, não realizado aqui.

Os três imports de userdata ainda caem no fallback. Retornar 1 para capacidade
sem implementar FS/restauração/escrita seria anunciar um serviço inexistente.

### Áudio

`play.html:463–486` cria AudioContext 48 kHz, estéreo, após gesto do usuário e
recebimento de `{memory,ring,capacity}`; carrega `audio-worklet.js`, conecta a
saída e retoma em novos gestos. CPAL substitui AudioContext/AudioWorklet; não
precisa copiar a restrição de autoplay do navegador.

`audio.rs` já consome o ring estéreo f32/48 kHz com validação, wrap, silêncio,
resampling, mute, conversão PCM e saída CPAL encerrável. Passaram testes offline
e uma saída física **muda**, sem escuta humana. Isso não prova áudio do jogo:
`wasm_has_page_js` retorna 1 no browser e 0 no Node; no Rust hoje cai em zero.
O bootstrap observado registrou `audMixerDeviceWasmDiscard`/“Headless mixer”.
A associação deve ser validada num teste da seleção do mixer; não basta mudar
a flag ou observar `audio_started=true`. Sem o worklet original, sua semântica
completa, inclusive campos auxiliares do header, permanece por conferir.

## 8. GPU: contratos que faltam conectar

### Ring e sincronização

`fiveplay-wgpu.js:13–35,2784–2958`: header de 64 bytes, head/tail em offsets de
bytes relativos, state em +8, contagens de comandos/erros em +12/+16 e sleeping
em +20. Comando = opcode u32, quantidade de words u32 e payload. WRAP volta a
64, sem contar como comando executado. `gpu_protocol.rs`/`drain.rs` já tratam
WRAP, limites de memória e acknowledgement após consumo.

| Operação | Original | Rust atual |
| --- | --- | --- |
| 0/2, 3–5, 11–17, 18–33, 37–38, 47 | recursos, uploads, estado e draws | subconjunto implementado; limitações abaixo |
| 1 | WRAP do consumidor | tratado no dreno |
| 6 | clear de profundidade/estêncil | erro explícito, ausente |
| 7 | blit da textura indicada ao canvas + submit + slots em voo | aceita opcode e submete; ignora textura para apresentação |
| 8 | submit, esperar conclusão GPU, escrever valor e notify no endereço | ausente; não pode concluir ao apenas ler o comando |
| 9/10 | screenshot/log | ausentes no dispatcher |
| 34–36 | copy region/resource, resolve MSAA | ausentes |
| 39–43 | compute, clear UAV, mips, readback, copiar contador | ausentes |
| 44–46 | create/begin/end query | ausentes |

Base Rust: `worker.rs:569–663`, `drain.rs:22–76`. O limite nativo de payload
de 128 KiB (`gpu_protocol.rs:67`) não é demonstrado pelo consumidor JS; conferir
o limite do produtor/trace real antes de afirmar compatibilidade de uploads.
Polling de 1 ms no dreno não reproduz o protocolo sleeping/notify nem o
processamento de conclusões assíncronas. Nunca deixar uma engine esperando
indefinidamente depois de o backend falhar.

### Recursos, shaders, bindings e draw

| Divergência concreta | Fonte original → implementação atual |
| --- | --- |
| Features/limits do device | JS pede recursos disponíveis e limites do adaptador (`477–525`); `surface.rs:32–38` usa defaults. Negociar BC, filtros, depth/stencil e limites usados, sem copiar flags Dawn de diagnóstico |
| Arrays de textura | JS usa `dim === 3 ? d : arraySize` (`549–568`); Rust lê ambos mas cria usando `depth_or_array_layers`, sem `array_layers` (`objects.rs:21–38`, `worker.rs:149–178`) |
| A8 e bytes finais de buffers | JS expande A8 em RGBA e faz escrita final alinhada (`601–632`); Rust não expande A8 e rejeita upload GPU cujo tamanho não é múltiplo de 4 (`286–303`) |
| Samplers | JS interpreta filtro, address, comparação, anisotropia e LOD (`205–220`); `worker.rs:439–444` cria sampler default e descarta o descritor |
| Aquisição de shaders | JS CREATE_SHADER agenda fonte/pack por hash e espera antes do draw (`309–475,635–645,2857`); Rust exige fonte previamente registrada (`404–430`). `gpu/shaders.rs` apenas extrai packs já disponíveis |
| Constantes inline | JS transforma tabela JSON em buffer no binding 15 (`442–475,1570–1574`); Rust faz o mesmo em `gpu/uniform.rs`, com tabela vazia zerada e ausência rejeitada |
| Uniformes | JS copia shadows por draw, `first * 16`, zeros de região ausente, offsets alinhados e slots em voo (`724–825`); Rust implementa snapshots por desenho, offset dinâmico e orçamento de 16 MiB por encoder, cobertos por `tests/uniforms.rs` |
| Storage/UAV | JS liga SRV em 32+, UAV em 160+, contadores em 176+ (`1557–1625,2151–2193`); Rust ignora esses tipos no bind group. SET_UAVS só aceita contadores iniciais `u32::MAX` |
| Views e recursos não ligados | JS escolhe dimensão pela declaração WGSL, usa dummy compatível e proxies de depth/stencil (`830–986`); Rust cria view antecipadamente pelo request, sem esses proxies/fallbacks (`338–397,795–825`) |
| Depth/MSAA | JS suporta pass só de profundidade, read-only flags e amostras do alvo (`1400–1439`); Rust exige alvo de cor, não liga depth e força `samples: 1`, `depth_format: None` (`711–775,892–965`) |
| Estado dinâmico | JS aplica scissor, blend constant e stencil reference (`1712–1818,2852–2856`); Rust guarda apenas IDs de blend/depth, armazena mas não aplica scissor (`518–553,980–1024`) |
| Vertex/instance IDs e limites | JS ajusta offsets quando shader lê IDs, fornece VB zero e limita draws aos buffers (`1744–1818`); Rust passa os starts diretamente e rejeita draw indireto (`843–855,988–1024`) |
| Cache/compilação | JS tem cache/receitas e variantes para interpolação/limites; Rust tem cache LRU limitado a 128 pipelines, com geração de shader na chave (`gpu/pipeline_cache.rs`); receitas e compilação assíncrona continuam ausentes |

As aproximações do original também precisam ser explícitas: border sampler e
MipLODBias não são reproduzidos integralmente, e occlusion visível vira 65536,
não uma contagem exata (`205–220,2029–2148`). O sample mask é armazenado no
estado JS, mas não aplicado ao descritor multisample (`1255,2852`). Não chamar
isso de D3D11 completo.

Queries gravam resultado low/high em +8/+12 e sequência de conclusão em +0,
por último; eventos concluem depois da fila GPU. Readback (`2344–2386,2861–2865`)
grava/submete a cópia na ordem e conclui o mapeamento depois, com row/slice pitch
do destino WASM e liberação da flag mesmo no tratamento de falha. Em wgpu,
callbacks e polling do device precisam progredir fora da thread bloqueada da
engine. A fila de comandos não pode depender de `main()` retornar.

### Apresentação não é submissão

`present(id)` (`2646–2768`) usa a textura **id**, redimensiona/configura o canvas
quando necessário, executa blit, submete, registra conclusão do slot de uniformes
e aguarda antes de reutilizá-lo. Nativo precisa adquirir surface texture, copiar
ou desenhar essa textura de origem, submeter e chamar `present()`, tratando
resize, minimização e perda da surface/device. `SurfaceContext` sozinho e o
teste de superfície não conectam isso ao comando PRESENT da engine.

## 9. Ordem mínima de implementação, quando liberada

1. **Capacidades + startup/input:** substituir argumento inválido pela escrita
   no bloco publicado; suportar escolha pendente e modo 3; testar janela/eventos
   e distinção explícita entre modo interativo e headless. Não habilitar flags
   de página/saves antes dos serviços correspondentes.
2. **ABI de host + FS/saves:** eliminar sucessos zerados indevidos; implementar
   os caminhos usados, errno, relógios, idioma, vida útil das pthreads, mailbox,
   proxy e saída. Fixtures memory64 de chamadas e efeitos na memória.
3. **GPU até frame correto:** device/recursos/fontes/constantes, bind groups,
   profundidade/estado e sincronização; ligar PRESENT a janela real no mesmo
   device. Fixture com uniformes alterados entre draws, sampler não default,
   textura array/A8, depth-only e confirmação da textura apresentada.
4. **Restante funcional:** copies/resolve, compute/UAV/counters, mips,
   queries/fence/readback com callbacks progredindo e dados/flags na ordem.
   Testes com endereços memory64 altos, wrap e crescimento, sem alocações
   proporcionais ao maior ID ou leituras integrais de arquivos enormes.
5. **IO e custo:** concorrência limitada, cache versionado e deduplicação;
   depois hints/read-ahead/bootset com prioridade de demanda e cancelamento.
   Medir antes de portar todos os caches/receitas do browser.
6. **Integração final:** executável/janela sem WebView/JS; áudio útil, input e
   save após reinício; shutdown sem threads penduradas. Só então considerar
   troca do launcher, preservando o atual durante o desenvolvimento.

Essas etapas ainda não foram executadas nesta fase de análise. Trocar apenas
`wasm_has_page_js` ou adicionar um blit não resolve os demais contratos.

## 10. O que foi realmente validado

- Antes de IO/áudio: fmt, Clippy release `-D warnings`, 102 testes do workspace
  (3 ignorados), build release, ring/surface e smoke Órbita 3/3. Não é GTA.
- Depois de IO/áudio e correções do host: biblioteca do player com 78 testes
  passando e 1 ignorado; integrações initialization/pthread/HTTPFS com 7 passando;
  saída CPAL física muda passou quando executada explicitamente.
- Bootstrap real autorizado: WASM de 63201802 bytes e SHA-256
  `11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0`;
  passou dos construtores e de MountAudioPacks e chegou à pré-carga de shaders.
  Watchdog interrompeu após 20 s; stack em `wasm_httpfs_pread`/`grcEffect::Preload`.
  `gpu_started=true io_started=true audio_started=true`; **nenhum frame GTA**.
- `game_host.rs:89–97` só rejeita algumas falhas de bootstrap e exige a flag GPU;
  ele aceita a interrupção do watchdog. Teste verde não comprova GPU consumindo
  comandos, conclusão da pré-carga, áudio útil, input nem jogabilidade.
- Nesta análise, repetido offline:
  `node tests/ram-worker-contract.mjs artifacts/io_worker.js` passou: original
  262144 bytes versus adaptado 4096 bytes, sem OPFS. É um contrato específico,
  não benchmark nem prova integral do worker.
- Cargo/RTK continuam indisponíveis no PATH e `.cargo` foi observada ausente.
  Não houve nova rodada Rust nem validação completa do workspace após IO/áudio.
  Compiladores em `.rustup` não substituem as dependências desaparecidas.

Entrega completa exige fmt, Clippy, testes offline, release e smoke do player;
o smoke GTA deve verificar bundle autorizado, frame apresentado, input causando
mudança observável e persistência após reinício. Essas condições continuam
pendentes. Referências, executáveis da raiz e launcher foram preservados.
