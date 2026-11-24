# Stress triplo: GTA V — 24/11/2026

## Protocolo

18 jatos Lazer em formação fixa à frente da câmera inicial (6 anteriormente), 36 explosões (12 anteriormente), resolução 1920×1200. Somente os veículos do teste ficam congelados/invencíveis durante a prova, para manter a carga gráfica; são excluídos ao terminar. Isso aumenta em 3x o número de objetos/eventos, não garante 3x o custo computacional nem replica a física da prova antiga. O cenário não é totalmente determinístico: população ambiente, clima e streaming variam.

O gate exige 18 entidades presentes e reportadas em tela pelo motor, ao menos cinco observações de carga completa após início das explosões, 36 explosões, encerramento do teste, 20 janelas de FPS, mínimo amostrado >=60 e nenhum intervalo de apresentação >16,667 ms. `IsEntityOnScreen` não comprova paridade visual ou ausência de oclusão. A telemetria não mede scanout físico.

## Resultados

| Rodada | FPS mínimo amostrado | Mediana | Pico | Intervalos >16,667 ms | Pior intervalo |
|---|---:|---:|---:|---:|---:|
| Base (4 workers, 32 passes) | 53,64 | 56,62 | 66,52 | 1046/1528 | 22,190 ms |
| 6 workers, 16 passes | 60,53 | 62,62 | 70,89 | 259/1678 | 20,635 ms |
| 8 workers, 12 passes | 52,66 | 56,58 | 65,11 | 1072/1499 | 41,228 ms |
| 6 workers, 16 passes sem diagnóstico | 73,59 | 76,51 | 84,54 | 6/2017 | 121,436 ms |
| 6 workers, 24 passes | 64,48 | 66,74 | 75,25 | 68/1779 | 32,309 ms |
| Cache local por epochs | 63,46 | 68,53 | 81,97 | 52/1826 | 66,389 ms |
| Perfil Tuned | 73,49 | 76,57 | 83,49 | 2/2019 | 18,967 ms |
| Repetição Tuned | 65,61 | 69,56 | 77,48 | 28/1832 | 18,739 ms |

Todas completaram a carga (18 existentes/em tela, 36 explosões, 17 observações de carga plena). Nenhuma aprovou o gate completo. A melhor repetição em FPS teve aproximadamente 13,18% de CPU média agregada em 20 CPUs lógicas, 4,79 GiB de RAM residente, 6,98 GiB privados e 2,27 GiB dedicados de GPU. A base teve 15,47% CPU, 4,87 GiB residente e 2,27 GiB dedicados. Carga de draws variou (aprox. 2478/frame na melhor repetição, 3624 na base); não atribuir todo o ganho à configuração. Sem comparação direta com Chromium na mesma cena. Zero erros de validação registrados não prova paridade visual; houve draws ignorados já existentes no runtime.

## Reprodução

```powershell
& scripts/Benchmark-Gta.ps1 -Stress -Tuned -Name stress3-nova-rodada -Fps 0
```

O perfil experimental ativa finalização paralela com 6 workers/16 passes, cache local de bindings, layouts de uniformes compartilhados e blocos de 4 MiB. Não ativa epochs, pré-compilação seletiva nem diagnóstico detalhado. As variáveis são aplicadas apenas ao processo de teste; as configurações normais permanecem inalteradas. `WGR_ENCODER_WORKERS` (2–8), `WGR_ENCODER_BATCH_PASSES` (8–128) e `WGR_LOCAL_BIND_EPOCHS=1` permitem experimentos. O último está desativado no perfil por não demonstrar ganho consistente.

Resultados e logs: `artifacts/perf-stress3-*-20261124-summary.json`, `*-resources.json`, `artifacts/stress3-*-20261124-{engine,output}.log`. O objetivo de 60 FPS sem nenhum frame lento, pico >100 FPS e qualidade visual integral ainda não foi validado.

## Rodada após c9778d3: fidelidade e sincronização

| Rodada | FPS mínimo amostrado | Mediana | Pico | Intervalos >16,667 ms | Pior intervalo | Draws ignorados no último log |
|---|---:|---:|---:|---:|---:|---:|
| Referência com diagnóstico de skips | 60,51 | 64,60 | 76,17 | 161/1739 | 25,462 ms | 1718 |
| Suporte a vértices procedurais | 64,52 | 67,55 | 77,62 | 77/1800 | 19,994 ms | 19 |
| Repetição com pipeline aquecido | 59,08 | 62,61 | 66,57 | 271/1652 | 117,448 ms | 0 |
| Chave de attachments por pass | 62,53 | 68,51 | 81,03 | 67/1819 | 32,621 ms | 0 |
| Entrega de conclusões de queries | 58,68 | 65,63 | 75,09 | 82/1718 | 21,573 ms | 0 |
| Repetição da sincronização | 51,73 | 59,67 | 71,20 | 766/1556 | 23,362 ms | 0 |

Todas as rodadas completaram a carga tripla e registraram zero erros de validação nos logs. A referência descartava `postfx::VS_BokehSprites_Adaptive` + `postfx::PS_BokehSprites` por ausência de input layout (o worker de referência do Chromium também descarta draws sem layout). A assinatura original WGSL usa somente `@builtin(vertex_index)`. O runtime passa a aceitar layout nulo somente para assinaturas de vértices comprovadamente procedurais, preservando offsets e usando um layout vazio reutilizável. O primeiro teste teve 19 skips enquanto o pipeline compilava; as repetições aquecidas registraram zero no último log. Isso não comprova equivalência visual pixel a pixel. Não comparar o FPS como se o trabalho gráfico fosse idêntico.

O sufixo da chave dos pipelines referente aos attachments é preparado por pass, preservando os formatos, slots vazios e flags de leitura. A entrega de conclusões de queries passou a ocorrer após uma única consulta ao dispositivo. O contador de batches pendentes só é decrementado após publicar os resultados para o guest, evitando que o worker entre na espera longa enquanto uma conclusão ainda não foi entregue. As duas rodadas da sincronização não registraram intervalos acima de 24 ms, mas não demonstraram ganho sustentado de FPS; a segunda teve aproximadamente 4046 draws/frame e 278 passes/frame na última janela. Ainda ocorreram leituras de assets acima de 200 ms. Nenhuma rodada aprovou o gate completo.

Validação: 45 testes unitários, duas fixtures GPU e três smoke tests locais aprovados. O executável foi recompilado. As mudanças desta rodada estão em `vendor/muguet/src/gpu.rs`; o perfil Tuned continua experimental. Os resultados não comprovam superioridade global sobre Chromium nem 60 FPS sem quedas.

## Distribuição e perfil padrão

Após solicitação explícita, o perfil testado foi incorporado aos defaults do runtime: 4 MiB por bloco, cache local de bindings, layouts compartilhados e lotes de 16 passes. A finalização paralela é ativada automaticamente a partir de oito CPUs lógicas (quatro workers entre 8 e 11; seis a partir de 12). Em CPUs menores permanece serial, salvo override explícito. As opções que não demonstraram ganho consistente (epochs e aquecimento por shader) continuam desativadas. Os resultados históricos acima usavam overrides antes desta mudança; o perfil de distribuição ainda requer validação em múltiplas GPUs e CPUs. A qualidade integral, o gate de 60 FPS e o pico acima de 100 FPS continuam objetivos em aberto.

Teste do perfil padrão, sem overrides de renderização: 18 jatos existentes/em tela, 36 explosões, 17 observações de carga plena; mínimo amostrado 63,65 FPS, mediana 68,63 FPS, máximo 78,12 FPS. Dos 1842 intervalos, 31 excederam 16,667 ms (pior 54,387 ms). Zero erros de validação e um draw ignorado durante compilação inicial no último log. O gate estrito ainda reprova. Os 46 testes unitários do runtime passaram. O pacote portable local passou na verificação dos checksums do ZIP e de ambos os executáveis. Relatórios: `artifacts/perf-stress3-default-profile-20261124-summary.json` e `artifacts/stress3-default-profile-20261124-engine.log`.

## Rodada após c381bf7: fases de frame e manutenção fora da thread gráfica

| Rodada | FPS mínimo amostrado | Mediana | Pico | Intervalos >16,667 ms | Pior intervalo |
|---|---:|---:|---:|---:|---:|
| Diagnóstico de fases, 16 passes | 75,52 | 78,48 | 87,51 | 1/2086 | 16,710 ms |
| Limite adicional de 256 draws | 80,48 | 82,63 | 90,53 | 1/2188 | 28,114 ms |
| 256 draws, estatísticas assíncronas | 70,52 | 72,52 | 81,70 | 3/1934 | 17,453 ms |
| Repetição sem diagnóstico de fases | 70,49 | 72,47 | 83,09 | 2/1934 | 17,159 ms |
| 128 draws, estatísticas assíncronas | 58,62 | 63,61 | 72,49 | 189/1699 | 36,603 ms |

As cinco rodadas confirmaram carga tripla completa (18 em tela, 36 explosões, 17 observações plenas). Todas reprovaram o gate estrito e terminaram com zero skips e zero erros de validação nos últimos logs. A medição de fases (`WGR_FRAME_PHASES=1`) é opcional e registra somente frames acima de 16667 us, inclusive a fase de entrada no mundo antes da janela oficial do benchmark. No teste de 256 draws, um frame de 28093 us teve 16769 us de manutenção, coincidindo com o log periódico de FPS. A coleta não demonstra causalidade isolada. O novo logger usa fila de oito tarefas com try_send; calcula memória e escreve estatísticas periódicas fora da thread GPU, mas mantém avisos/erros e diagnósticos detalhados síncronos. Saturação pode descartar estatísticas informativas e é contabilizada no próximo relatório. `WGR_ASYNC_STATS=0` restaura o comportamento anterior. O limite `WGR_ENCODER_BATCH_DRAWS=256` permanece experimental, desativado por padrão; só divide no fim de um pass e preserva ordem de submissão e validação.

Após a mudança, as duas sessões de 256 draws tinham cerca de 3600 draws/frame na última janela, contra 2606 no teste de maior FPS; não comparar como replay idêntico. A CPU média agregada variou entre 14,13% e 14,66%, sem ganho sustentado comprovado. A melhor execução em FPS registrou aproximadamente 4,94 GiB residentes e 2,27 GiB de VRAM dedicada. Não foi feita redução deliberada de qualidade nem comparação de pixels com Chromium. A configuração de 128 draws não foi promovida. Validação final: 47 testes unitários e três smoke tests locais passaram; binário recompilado e instalado. Os smoke tests são da fixture Órbita. A CI remota do commit anterior ainda estava em andamento ao fechar a rodada. Logs: `artifacts/stress3-{frame-phases,draw-batches256,async-stats,async-repeat,draw-batches128}-20261124-{engine,output}.log`; resumos e recursos com prefixo `artifacts/perf-`.
