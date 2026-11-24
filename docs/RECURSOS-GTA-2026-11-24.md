# Consumo de recursos — GTA V, 24/11/2026

## Protocolo

Após 7fff5b5: 18 jatos em tela, 36 explosões, 1920×1200, sem limite de FPS, perfil Tuned com seis workers gráficos, 16 passes e limite experimental de 256 draws. As seis rodadas confirmaram a carga completa (17 observações plenas cada). Nenhuma cumpriu o gate de zero intervalos acima de 16,667 ms. Não houve redução deliberada de qualidade. O cenário ambiente e os draws variam entre sessões, portanto os resultados não são replay idêntico nem comprovam causalidade de cada mudança.

`scripts/Benchmark-Gta.ps1 -Stress -Tuned -ResourceCounters -Name nome-unico -Fps 0` coleta CPU normalizada por CPUs lógicas em intervalos pós-mundo com processos correspondentes, estimativa de ms-núcleo/frame, memória do processo, GPU por motor e memória dedicada/compartilhada, I/O do processo, disco físico e paginação do sistema. Registra também deltas dos contadores de assets. O sampler levou cerca de 1,1 s por coleta, com cinco segundos de espera entre coletas; pode influenciar o desempenho. GPU e disco físico têm apenas 4–5 amostras pós-mundo por rodada; ausência de dados não é zero. I/O do processo inclui rede/dispositivos, não apenas disco. CPU/frame é estimativa com FPS médio da janela oficial, não trace por thread. Maior motor GPU por amostra não é soma de motores. Consultas de referência: https://learn.microsoft.com/en-us/windows/pix/timing-captures/timing-captures-overview ; https://learn.microsoft.com/en-us/previous-versions/aa394323%28v%3Dvs.85%29 ; https://learn.microsoft.com/en-us/windows/pix/timing-captures/win32-fileio .

## Resultados

| Rodada | FPS mínimo amostrado | Mediana | Mundo (s) | CPU (%) | CPU ms-núcleo/frame | RAM residente pico (GiB) | VRAM dedicada pico (GiB) | I/O leitura processo (MiB/s) | Frames lentos |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Referência | 62,57 | 67,57 | 16,89 | 16,32 | 47,57 | 4,93 | 2,27 | 4,80 | 25/1796 |
| Disk singleflight | 58,58 | 67,57 | 14,82 | 15,68 | 46,82 | 4,73 | 2,02 | 2,97 | 59/1755 |
| Repetição | 54,66 | 62,65 | 14,79 | 16,51 | 52,88 | 5,00 | 2,27 | 4,05 | 516/1636 |
| Cache quente 128 MiB | 51,57 | 54,50 | 16,79 | 14,27 | 51,72 | 4,96 | 2,03 | 1,19 | 1274/1444 |
| Receitas compactas | 68,61 | 71,68 | 15,75 | 17,13 | 46,73 | 4,93 | 2,27 | 11,10 | 1/1920 |
| Compactas + 16 workers I/O | 71,60 | 76,26 | 15,50 | 17,10 | 44,61 | 4,89 | 2,27 | 8,55 | 4/2007 |

O maior intervalo da rodada compacta foi 16,691 ms; com I/O16, 18,882 ms. A média amostrada do motor GPU mais ocupado foi 23,25% na referência e 19% na rodada I/O16. O disco físico global variou de 0,25 a 24,46 MiB/s de leitura; houve picos de paginação global de até 29.920 páginas/s. Não atribuir a atividade global apenas ao WGR. A última janela teve 3423 draws/frame na referência e 2905 na rodada I/O16; a comparação não prova eficiência causal por frame. Nenhum resultado justifica reduzir o cache quente padrão de 256 MiB nem promover I/O16 ainda.

## Alterações e evidências isoladas

- `WGR_DISK_SINGLEFLIGHT=1`: lock estriado por chave somente no caminho de disco, reconsulta do cache quente após obter o lock. Evitou 2005 e 2094 leituras duplicadas nos dois testes, mas não mostrou ganho consistente no FPS; permanece desligado. Mantém verificação SHA-256 na leitura do disco e cache por origem/versão. A atualização LRU do bloco já mais recente é eliminada sem alterar a ordem de expulsão.
- Catálogo compacto: fila de `Box<str>` em vez de árvores `serde_json::Value`; decodificação ao consumir receita, ordem e hashes preservados, limite de leitura 33 MiB e 256 KiB por registro. Catálogo fora do limite é apenas dica descartável; compila sob demanda sem apagar o arquivo. A política shader-aware experimental pode fazer uma decodificação adicional. Mantém todas as receitas válidas do catálogo atual. A fixture local contou alocações solicitadas ao allocator, não RSS ou VRAM: 34139 registros, 34139 válidos, fila antiga 349492451 bytes, nova 22626733 bytes (93,53% menos; 311,72 MiB economizados). Construção 234,0983 ms vs 6,937 ms; decodificação diferida 187,2778 ms. O custo de ler arquivo e o índice de hashes estão fora da fixture; o pico total do jogo não teve redução equivalente comprovada. A fila compacta e a atualização LRU exata permanecem no runtime por padrão.
- Validação final: 49 testes unitários, fixture de memória real e três smoke tests Órbita aprovados; build release instalado. Sem execução GTA após o pequeno ajuste final de limite de leitura/terminadores; as sessões anteriores usavam a mesma representação compacta. Nenhum commit desta rodada.

Reprodução da fixture (PowerShell, na raiz): `$env:WGR_RECIPE_FIXTURE=Join-Path $env:LOCALAPPDATA 'WGR/native-rage/metadata/pipelines-v1.jsonl'; cargo test --manifest-path vendor/muguet/Cargo.toml --release --locked --target-dir vendor/muguet/target --test recipe_memory -- --ignored --nocapture`. Sem variável, usa registros sintéticos originais. Artefatos locais: `artifacts/perf-resource3-{baseline,singleflight,singleflight-repeat,hot128,compact,compact-io16}-20261124-{summary,resources}.json` e `artifacts/resource3-*-20261124-{engine,output}.log`.

## Rodada após d789812: ring de uniformes

| Configuração | FPS mínimo | Mediana | Pico | Staging CPU no último log (MiB) | Capacidade ring GPU (MiB) | CPU ms-núcleo/frame | Frames lentos |
|---|---:|---:|---:|---:|---:|---:|---:|
| Staging compartilhado, 3 slots | 57,71 | 62,70 | 76,90 | 8,50 | 36 | 50,93 | 334/1677 |
| Controle, 3 slots | 67,49 | 68,62 | 80,76 | 24,75 | 36 | 45,87 | 3/1867 |
| 2 slots, staging separado | 57,70 | 64,56 | 76,52 | 18,00 | 24 | 53,32 | 346/1699 |

Todos confirmaram 18 jatos em tela, 36 explosões e 17 observações plenas, com zero skips e zero erros de validação no último log. Mesmo perfil Tuned, 256 draws, 16 workers I/O. Não são cenas idênticas: última janela teve 3664, 3356 e 3647 draws/frame, respectivamente. A economia observada de staging compartilhado foi 16,25 MiB (65,66%) e a capacidade do ring de GPU com dois slots caiu 12 MiB (33,33%). Isso não implica economia equivalente de RAM ou VRAM total. RAM residente pico: 4,94/4,88/4,97 GiB; VRAM dedicada pico aproximadamente 2,27 GiB nas três. Mundo: 15,331/14,132/13,681 s. Pior intervalo: 21,519/17,401/20,563 ms. Nenhuma sessão passou no gate estrito. Não promover nenhuma das duas opções: ambas mostraram menor FPS que o controle; manter três slots e staging separado como padrão.

`WGR_SHARED_CPU_STAGE=1` recicla vetores da CPU após flush de todos os ranges sujos, preservando offsets físicos e fences. O contrato de `Queue::write_buffer` confirma cópia imediata da origem em memória de staging (https://docs.rs/wgpu/latest/wgpu/struct.Queue.html#method.write_buffer). Fixture física `shared_cpu_stage_preserves_ordered_uploads` verificou todos os bytes de 12 uploads alternando três regiões com fences, sobrescrevendo a origem antes de submit. A fixture valida o contrato e a ordem, não todos os caminhos do runtime. `WGR_UNIFORM_FRAME_SLOTS=2|3` controla a quantidade de regiões, e `WGR_RING_STATS=1` registra capacidade do staging e do ring independentemente. Ambos os experimentos estão opt-in e não alteram qualidade deliberadamente. Artefatos: `artifacts/perf-resource3-{shared-stage,stage-control,ring2}-20261124-{summary,resources}.json` e logs com prefixo `artifacts/resource3-`.

## Finalização antecipada de encoders

`WGR_EARLY_ENCODER_FINISH=1` permite finalizar lotes em workers enquanto a thread gráfica grava os seguintes. A fila continua limitada e a submissão mantém a ordem de gravação. A opção permanece desligada por padrão.

Fixture Vulkan na RTX 4060 Laptop: 320 passes/3200 draws, mediana de finalização 0,693/0,490 ms (controle/antecipada), tempo total 2,242/2,125 ms; 800 passes/3200 draws, finalização 2,130/1,327 ms e tempo total 5,175/4,573 ms. O pixel de referência coincidiu e os três testes físicos passaram. A fixture não representa toda a carga do GTA.

| Métrica GTA | Controle | Antecipada |
|---|---:|---:|
| FPS mínimo amostrado | 69,51 | 66,64 |
| FPS mediano | 73,63 | 68,62 |
| FPS máximo | 82,86 | 73,59 |
| Frames lentos | 1/1936 | 38/1810 |
| Pior intervalo (ms) | 17,265 | 20,423 |
| CPU (%) | 16,93 | 17,06 |
| CPU ms-núcleo/frame | 45,74 | 49,26 |
| Mundo (s) | 24,658 | 25,102 |
| RAM residente pico (GiB) | 4,98 | 5,09 |
| VRAM dedicada pico (GiB) | 2,27 | 2,03 |

Ambas as sessões confirmaram carga tripla completa, mas reprovaram o gate estrito. A pressão de memória e o streaming variaram entre sessões; a redução de VRAM não comprova causalidade. O ganho sintético não se traduziu em ganho de FPS nesta comparação. Manter o controle como padrão. Resumos: `artifacts/perf-resource3-early-{control,finish}-20261124-summary.json`.
