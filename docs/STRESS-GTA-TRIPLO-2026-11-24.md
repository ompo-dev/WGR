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
