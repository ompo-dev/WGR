# WGR / GTA V — auditoria de renderização, streaming e desempenho

Data: 23/11/2026 · Plataforma de ensaio: Windows, RTX 4060, Vulkan · Resolução interna: 1920 × 1200, escala 100%.

## 1. Como o sistema funciona de ponta a ponta

1. **Interface do WGR**: `crates/wgr-app`, GPUI e os componentes já existentes do MomorRust. Mostra jogos, fontes, diagnósticos e configurações, mas **não renderiza diretamente o GTA**.
2. **Launcher e processo filho**: `crates/wgr-app/src/native_child.rs` valida a seleção do port e inicia o `wgr-rage.exe` ao lado do próprio `WGR.exe`, com uma janela-filho e canal de controle. O launcher controla foco, redimensionamento, teclado e encerramento.
3. **CPU/Wasm**: `vendor/muguet/src/main.rs`, `app.rs` e o Wasmtime carregam uma build Wasm autorizada e executam a engine. A compilação JIT pode ser serializada e reutilizada como `.cwasm`; não alterar ou invalidar esse arquivo durante uma sessão.
4. **I/O e streaming**: `vendor/muguet/src/io.rs` consome até 30 leituras simultâneas exigidas pela engine e uma fila separada de dicas de pré-carregamento. `remote.rs` valida versões/origens dos arquivos, usa HTTP Range, deduplica blocos e os guarda em cache RAM/SQLite/disco. Prioridade: leitura necessária para o jogo, dica urgente, pré-busca especulativa.
5. **GPU**: `vendor/muguet/src/gpu.rs` traduz comandos gráficos da engine para wgpu/Vulkan. Há bind groups, pipelines de renderização e compute, recursos e cópias entre buffers. Readbacks, passes e submissões ao driver têm custo de CPU. O suporte de oclusão deve preservar geometria caso um shader ainda não esteja pronto.
6. **Apresentação/controle**: a imagem é apresentada na janela do runtime. O FPS registrado é o número comunicado pela engine/renderizador; **não é garantia de frames únicos exibidos nem de fluidez na movimentação**.

## 2. Causa observada da cena cinza e FPS ilusório

Uma captura inicial chegou a mostrar apenas o céu, alguns objetos suspensos, chão cinza e o jogador preto. Repetições prolongadas com o mesmo runtime mostraram terreno e construções completos posteriormente, com zero erros de validação GPU reportados em algumas janelas. A hipótese principal é **streaming incompleto**, agravado por muitas dicas repetidas e longas operações de cache, mas não há prova de que essa seja a única causa: shaders e queries de oclusão continuam como hipóteses a investigar em cenas reproduzíveis.

Nunca aceitar FPS alto sem textura, geometria, iluminação e interação completas como uma melhoria. O objetivo é reduzir **tempo até o mundo estar visualmente utilizável**, P10/P1 de FPS, latência de input e tempo de transição entre áreas, além da média.

## 3. Diagnóstico do sistema original

No ensaio `artifacts/gta-regression-repro-20261123/` (versão anterior): mundo detectado após **70,727 s**; 211 amostras após mundo+20 s, FPS P10 **44,71**, mediana **46,72**. Cerca de **394.939** dicas de pré-carga recusadas e **184.545** duplicadas internamente. Existiam leituras remotas com centenas de milissegundos. A carga gráfica inclui milhares de draws e centenas de passes por frame; a GPU não necessariamente está saturada. Esse teste já usava cache persistente que variou entre rodadas.

## 4. Alterações realizadas neste ciclo

### Deduplicação próxima à origem (`vendor/muguet/src/io.rs`)

- Dicas são identificadas pelo arquivo, blocos de 128 KiB, extensão e prioridade sem construir caminho de texto nem disputar locks do transporte.
- Dicas urgentes repetidas são admitidas novamente depois de 50 ms, e especulativas depois de 350 ms; essas janelas não descartam permanentemente nenhuma leitura. A fila interna mantém seus próprios controles.
- Contador `hints_source_coalesced` permite verificar que a economia ocorre antes dos estágios caros.

### Cache de memória (`vendor/muguet/src/remote.rs`)

- Orçamento padrão expandido de 48 para **128 MiB**; override explícito por `WGR_ASSET_HOT_CACHE_MIB` entre 48 e 256 MiB.
- LRU ordenada com `HashMap` e `BTreeMap` evita varrer todos os objetos para descobrir qual bloco descartar, mantendo as chaves e a versão/origem verificadas.
- Dicas especulativas podem ocupar um único slot quando há menos de 12 leituras vivas; as leituras exigidas pelo jogo não esperam por essa política.

### Cache SQLite (`crates/wgr-core/src/assets.rs`)

- Contador transacional `cache_totals` substitui `SUM(size)` sobre toda a tabela a cada inclusão de bloco. Uma migração segura calcula o total das tabelas preexistentes apenas na criação do contador.
- Triggers preservam os valores durante adição, substituição, exclusão por capacidade e remoção de objetos inválidos. Os arquivos do cache continuam verificados por SHA-256; nenhuma verificação foi removida.
- Uma janela remota de até 1 MiB agora pode escrever até oito blocos de 128 KiB em **uma única transação SQLite** (`put_batch`), em vez de oito commits isolados. Blocos de mesmo conteúdo compartilham hash. Um teste adicional cobre cota, remapeamento, reabertura e objetos protegidos contra expulsão.
- Leitores do cache obtêm a referência pelo índice, verificam o arquivo e seu SHA-256 **fora do lock do SQLite**, e revalidam a referência antes de devolver os bytes. Isso permite hashes em paralelo, impedindo que uma leitura longa bloqueie todas as demais. A trava externa do cache passou a `RwLock` e o banco preserva seu próprio `Mutex` de transações.
- Em caso de corrida com expulsão, remapeamento ou arquivo danificado, os dados não são retornados se o hash indexado mudou; o código não remove a nova referência do outro leitor/escritor.

### Transporte DNS (`crates/wgr-core/src/security.rs`)

- Reutiliza por até **30 s** o cliente e os IPs públicos que passaram pela validação de origem; só o cliente de assets explicitamente autorizado ativa essa opção.
- Outros clientes continuam resolvendo DNS por solicitação, como antes. Expirado o prazo, revalida DNS. Redirecionamentos continuam passando pelas validações originais.
- Novas métricas: `dns_lookups`, `dns_cache_hits`, tempos acumulados de leitura/gravação de cache e de espera por rede.

### Redução de custo por draw (`vendor/muguet/src/gpu.rs`)

- Strides dos 16 vertex streams calculados num array de tamanho fixo em vez de `Vec` novo por draw.
- Chaves temporárias de pipeline usam `SmallVec<[i64; 48]>`; o vetor tradicional é materializado apenas quando um estado novo precisa ser inserido em cache. Os valores, a ordem e o resultado da chave foram preservados.
- Variáveis de diagnóstico que antes consultavam o ambiente do Windows a cada draw agora são lidas uma única vez. Coletas JSON detalhadas de personagens foram restringidas a `MUGUET_PED_PROBE=1` ou `MUGUET_TEXTURE_DIAGNOSTICS`, pois geravam alocações e cresciam durante partidas longas. A verificação de rótulos de shaders de personagens é sem alocação.
- `PassInfo` não é mais clonado em cada draw para obter apenas a largura/altura; a lista temporária de vertex buffers foi convertida para `SmallVec`. `collect_pipelines` não consulta a fila do compilador quando não há compilação pendente.

### Readbacks e custo de submissão CPU–GPU

- Medições instrumentadas: cerca de **4 leituras de textura por frame**, algumas janelas consumindo **5,7 a 9,5 ms/frame de readback**, com 5 ou mais `queue.submit` por frame. Sem otimização, isso é custo de sincronização e driver, não somente upload de texturas.
- Reutilização de buffers de leitura após `map_async`/`unmap`, com limite de retenção de **8 MiB**, até **48 buffers** e até **1 MiB por item**. Os buffers permanecem exclusivos durante processamento de GPU e envio ao jogo, evitando ler dados sobrescritos. O perfil mostrou **2.122 acertos e zero falhas** em uma janela estável de 523 frames.
- O agrupamento de readbacks foi testado temporariamente com `MUGUET_BATCH_READBACK=1`, mas **revertido do código entregue**. Um ensaio teve P10 de **26,78 FPS** e mediana de **27,85 FPS** numa região urbana mais pesada, além de esperas de leitura observadas; a versão imediata voltou a superar 60 FPS em janelas de carga comparável. A redução isolada do custo de readback não comprova maior FPS nem menor latência. Manter a ordem e a disponibilidade dos dados é mais importante que cortar submits a qualquer custo.
- As capturas examinadas com e sem agrupamento apresentaram cenário detalhado e texturas carregadas; isso não substitui comparação A/B da mesma posição e trajetória, nem validação de latência do input.

## 5. Primeiras medições — observar diferenças de cenário/cache

| Sessão | Mundo (s) | FPS P10 | FPS mediano | Observação |
| --- | ---: | ---: | ---: | --- |
| Baseline, commit `6beac69` | 70,73 | 44,71 | 46,72 | 211 amostras, original |
| Dicas + RAM 128 MiB | 21,82 | 35,81 | 46,63 | 230 amostras, cena e streaming variáveis |
| Mais SQLite O(1) + DNS TTL 30 s | 15,09 | 45,70 | 49,66 | 233 amostras, novos caches aquecidos |
| Mais arrays/SmallVec inline para chaves de pipeline | 26,84 | 45,69 | 51,61 | 230 amostras, cidade diferente |
| Mais gravação SQLite de blocos em lote | 37,72 | 42,74 | 46,71 | 204 amostras, cena diferente |
| Leitura de cache concorrente | 21,16 | 43,78 | 48,70 | 240 amostras, cenário variável |
| GPU sem diagnósticos contínuos | 30,61 | 48,74 | 53,72 | 240 amostras, instrumentação ativada |
| Pool de readbacks, modo imediato | 19,43 | 50,76 | 54,61 | 218 amostras, instrumentação ativada |
| Agrupamento de readbacks **experimental** | 42,89 | 26,78 | 27,85 | 125 amostras, região central urbana |
| Mesma build, readback imediato restaurado | 33,27 | 55,63 | 65,58 | 164 amostras, cenário urbano, agrupamento desativado |

Essas medições foram realizadas em sessões **diferentes do GTA**, com posições, ambiente, trajetória e quantidade de dados já baixados variáveis. Logo, os tempos de inicialização **não provam um ganho percentual causal**, e os P10 não são estritamente comparáveis.

As sessões adicionais confirmam que o cache em lote e as chaves de pipeline não causaram desaparecimento sistemático de texturas nas capturas examinadas. Não provam aumento constante de FPS; a comparação controlada de cenário/posição ainda está pendente. O teste de leitura concorrente foi concluído com 240 amostras. O agrupamento de readbacks reduziu o trabalho de submissão no perfil, mas ficou associado a esperas e menor desempenho na cena estudada; foi descartado em favor do caminho imediato.

### Verificação visual final e limitação ainda aberta

- O smoke da build final (`artifacts/gta-final-verified-20261123/`) registrou o primeiro estado `world_confirmed=true` aos **64,26 s**, mas sua captura depois de ~95 s ainda mostrou chão cinza e construções ausentes, apesar de FPS > 40. **Isso não é uma cena pronta para jogar e impede declarar a regressão visual resolvida.**
- Em outra execução da mesma build (`artifacts/gta-final-visual-soak-20261123/`), o mundo foi detectado após **19,42 s** e uma captura com aproximadamente 40 s de execução mostrou rua, veículos, personagem, edificações, sombras e vegetação corretamente. O problema parece dependente de região/ordem de carregamento; a causa exata permanece **não confirmada**.
- Todos os testes de integridade do cache, shader e engine passaram, mas o `world_confirmed` atual só confirma o evento da engine, **não** uma inspeção visual independente de completude de texturas/meshes. O próximo aceite deve comparar cenários fixos com cache frio/quente, usar capturas ao longo do tempo, acompanhar hints e verificar queries de oclusão e shaders se o cenário continuar incompleto.

## 6. Restrições e próximas fronteiras de engenharia

- **Qualidade**: não diminuir resolução, sombra, distância de visão, texturas nem omitir objetos. Não considerar FPS alto em cenários incompletos como sucesso.
- **GPU/driver**: medir draws, passes, submissões, sincronização por readback e tempo efetivo do driver em cenas repetíveis. Evitar readbacks diferidos não aprovados anteriormente: causaram regressões graves. Preferir reaproveitamento de recursos por estado.
- **Streaming**: perfil de latências P50/P95/P99 por arquivo, cache quente/frio e posição geográfica fixa; diferenciar upload de textura de tempo de rede.
- **Cache**: medir a serialização da leitura SQLite, integridade SHA, escrita de blocos e frequência de checkpoint WAL; preservar os limites de espaço livre e a origem dos dados.
- **Wasmtime**: o código já reutiliza módulos compilados; confirmar custos de inicialização antes de experimentar outro alocador. Não presumir que pooling melhorará um único jogo com memória compartilhada grande.
- **Controles e FPS real**: automatizar movimento e explosões em cenário determinístico, capturar frames e medir frames efetivamente apresentados, P10/P1, frametime P95/P99 e input-to-photon. O smoke automático do jogo atualmente não confirma todos esses critérios.

Documentação oficial consultada:

- Wgpu 30, boas práticas: https://wgpu.rs/doc/wgpu/documentation/best_practices/dos_and_donts/index.html
- Vulkan, profiling: https://docs.vulkan.org/guide/latest/profiling.html
- Wasmtime, compilação antecipada: https://docs.wasmtime.dev/examples-fast-compilation.html
- SQLite, WAL e checkpoints: https://www.sqlite.org/wal.html

Não houve mudança na UI MomorRust e não houve alteração de saves dos usuários neste ciclo. Relatórios e capturas de teste ficam sob `artifacts/`, não devem entrar no commit.
