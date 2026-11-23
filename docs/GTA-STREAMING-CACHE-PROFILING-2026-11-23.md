# GTA V no WGR — perfil e redução da contenção do cache

## Objetivo e restrições

Otimizar a inicialização, o streaming de objetos e a latência do cache nativo sem diminuir resolução, sombras, texturas, distância de visão ou omitir geometria. Não apagar dados do usuário nem usar FPS de cenários incompletos como evidência de melhoria.

Esta rodada parte do commit `7e79295` e mantém o tratamento conservador das queries de oclusão. Os relatórios e as capturas de execução ficam em `artifacts/`, fora do Git.

## Problemas encontrados

1. **Atualização síncrona de recência em todas as leituras verificadas.** Mesmo com SHA-256 calculado fora do mutex SQLite, cada hit executava um `UPDATE objects SET last_access...` no caminho crítico de uma das 32 threads de I/O.
2. **Granularidade de segundos nos timestamps do LRU.** Em uma rajada de carregamento, centenas de blocos tinham a mesma recência. Isso permitia desempate por hash e expulsão de objetos acabados de ler. O teste de diminuição do orçamento do cache reproduziu essa falha.
3. **Reabertura desnecessária do SQLite durante a inicialização.** A função `configure_budget` destruía e recriava o `DiskCache` para atualizar o limite de armazenamento, mesmo com o banco já configurado e o prefetch de cabeçalhos RPF em atividade.
4. **Syscall de metadados redundante por bloco.** A leitura já limita os bytes com `take(limit + 1)` e rejeita objetos excedentes; consultar `File::metadata` antes causava outra chamada ao sistema de arquivos.

## Correções feitas

- O LRU acumula no máximo 64 objetos distintos e atualiza `last_access` **em uma transação curta**, em vez de executar uma escrita por hit. Os acessos pendentes sempre são sincronizados antes de qualquer análise de expulsão por orçamento; em encerramento limpo, o último lote parcial é persistido.
- Cada acesso recebe uma marca de recência **monotônica em microssegundos**, inclusive dentro do lote. Os valores antigos em segundos continuam válidos no SQLite: são naturalmente mais antigos que os novos acessos, sem migração destrutiva.
- `DiskCache::set_limit` atualiza o orçamento em memória preservando a conexão, os contadores e o estado já preparado. Os novos writes continuam aplicando o limite como antes.
- As leituras não fazem mais `metadata()` antes do `read_to_end` limitado; SHA-256 e o limite de tamanho continuam obrigatórios. Pré-alocação limitada a 128 KiB reduz realocações dos blocos comuns.
- Novos contadores no relatório de I/O: `disk_db_wait_ms`, `disk_verify_ms`, `disk_lru_batches`, `disk_lru_entries`, além dos indicadores anteriores. São tempos **somados entre as threads**, não tempo de parede.

## Benchmarks e comparação

Um benchmark **sintético, ignorado pela CI normal**, executou 7.200 leituras verificadas com 16 workers em um cache temporário. O código de referência de atualização LRU individual realizou a carga entre **0,28 e 0,62 s**, conforme o aquecimento das leituras; o código com lotes executou a mesma carga em **0,20 s** nas duas medições. O ganho desta microcarga não prova aceleração do GTA: antivírus, disco, shader compilation, spawn e cache frio continuam afetando a partida.

| Execução GTA | Spawn (x, y) | Mundo detectado | FPS P10 | FPS mediano | Amostras |
| --- | --- | ---: | ---: | ---: | ---: |
| Antes, `7e79295` | -16,76; -1453,15 | 69,41 s | 50,71 | 55,63 | 238 |
| LRU em lotes antes de corrigir precisão/reabertura | 128,54; -1307,69 | 173,57 s | 52,71 | 57,61 | 240 |
| LRU monotônico + atualização de orçamento sem reabertura + leitura limitada | 94,95; -742,96 | 34,07 s | 49,64 | 54,61 | 240 |

**As três execuções são de regiões e estados de disco/cache diferentes.** Não usar diferenças percentuais na tabela como causalidade. Para comparar renderização, é necessária a mesma posição, câmera, trajetória e janela pós-carregamento, além de cache frio e quente separados. O tempo até `world_confirmed` não garante o cenário visualmente completo.

O primeiro perfil do LRU em lote registrou cerca de **13,3 mil hits de disco**; dos aproximadamente **462 mil ms** de `disk_lookup_ms`, `disk_verify_ms` somou **281 mil ms** e a espera pelo mutex SQLite somou **86,6 mil ms**. Uma parte das verificações também ocorre na preparação de shaders fora do contador `disk_lookup_ms`, portanto os tempos não devem ser subtraídos uns dos outros como categorias exaustivas.

Na versão final dessa rodada, o perfil registrou **13.843 hits de disco**, **135.200 ms** em `disk_lookup_ms`, **20.898 ms** em espera pelo mutex e **125.623 ms** na leitura/verificação, com **202 lotes de LRU** e **11.843 entradas de recência processadas**. O valor `disk_verify_ms` inclui chamadas de preparação de shaders que não entram em `disk_lookup_ms`. São tempos acumulados em leitores concorrentes, não durações de carregamento físico.

Em execução anterior **no mesmo ponto inicial** de `(94,95, -742,96, 45,75)` (`artifacts/occlusion-safe-second-smoke-20261123/`), o evento `world_confirmed` aconteceu em **125,29 s**; na versão final ocorreu em **34,07 s**. O cache, a memória do sistema e as condições do experimento estavam diferentes. **Não atribuir a diferença diretamente ao patch sem A/B com controle de estado do cache.** A cena final apresentou pavimentação, prédios, veículos, vegetação e iluminação nas capturas `capture-world.png` e `capture-long.png`, sem flags de redução de qualidade.

## Testes de integridade e limites

- Testes de orçamento, corrupção, SHA-256, concorrência de leituras/escritas e expulsão: obrigatórios antes de entrega.
- Teste adicional exige que um bloco recém-lido seja preservado após reduzir o orçamento e gravar outro objeto.
- Nenhuma alteração na UI MomorRust, nos saves, nas opções de qualidade, no tamanho padrão do cache RAM (128 MiB) ou nas URLs autorizadas.
- Não declarar 100 FPS sustentados sem validar deslocamento acelerado, explosões, cenas urbanas exigentes e frame-time P95/P99.
- `scripts/Test-NativeInput.ps1` não conseguiu trazer a janela à frente no Windows; o teste interrompeu-se antes de confirmar movimento e mouse. Isso não equivale a falha de input da engine, mas o teste real de movimentação **permanece pendente**.
- O problema visual reproduzido na região `(-59,74, -1110,62, 26,44)` não foi demonstrado como definitivamente resolvido: a seleção de spawn ainda não é determinística.

## Validação da entrega

- Testes do `wgr-core`: **16 aprovados**, 1 benchmark manual ignorado; teste adicional de catálogo aprovado.
- Testes do runtime nativo `wgr-rage`: **36 aprovados**; suíte do workspace e Clippy sem erros de correção; binários release gerados e SHA-256 conferido nas cópias instaladas.
- Smoke da fixture local do player: **3/3 execuções aprovadas**, mediana do primeiro frame de **90 ms**, save restaurado, sem testar gamepad físico ou audição humana. A fixture **não mede FPS do GTA**.
- O GTA foi testado sem alterar as opções gráficas; tempo ao mundo de 34,07 s, P10 de 49,64 FPS e mediana de 54,61 FPS após aquecimento, mas o experimento não demonstra FPS sustentado de 100 nem melhora causal.
- Entradas físicas de teclado/mouse no GTA continuam sem comprovação nesta rodada: a rotina de teste não conseguiu assumir o foco da janela.
