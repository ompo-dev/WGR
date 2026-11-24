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
