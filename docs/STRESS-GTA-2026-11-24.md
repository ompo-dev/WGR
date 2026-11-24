# Stress GTA — 24/11/2026

1920×1200, RTX 4060 Laptop, i7-13650HX, 6 jatos e 12 explosões, FPS de submissão reportado pelo port. O teste ainda não atende a meta de 60 FPS mínimos. Cenas variam entre execuções; não atribuir causalidade isolada.

| Rodada | Mediana | Mínimo | P10 | Pico |
| --- | ---: | ---: | ---: | ---: |
| 8 cores guest / 12 workers I/O | 43,74 | 31,80 | 32,81 | 61,20 |
| Estado gráfico otimizado | 50,68 | 42,72 | 47,69 | 84,93 |
| Repetição | 44,74 | 38,76 | 40,67 | 61,56 |
| Cache 384 MiB | 47,73 | 38,75 | 40,76 | 60,65 |
| Frame pacing instrumentado | 24,85 | 13,90 | 18,89 | 56,25 |
| Retenção adaptativa de bind groups | 42,75 | 19,90 | 25,83 | 66,66 |

Seleção explícita de P-cores: teste interrompido sem mundo confirmado após mais de 120 s; código revertido. Retenção adaptativa revertida; cache padrão 256 MiB, guest 4 cores e I/O 32 workers mantidos. Overrides de orçamento de threads permitem investigação futura, não são padrão.

Mantidos: arrays fixos para estado gráfico; chave de attachments recalculada apenas quando necessário; arrays inline nos passes; menor clonagem de handles SRV; telemetria de frame pacing opt-in, ativada no benchmark de stress. A telemetria mede intervalos entre chamadas de apresentação, não scanout físico. Na execução instrumentada, 752 de 766 intervalos excederam 16,667 ms, máximo 111,235 ms. Na tentativa de retenção adaptativa, 884 de 1125 excederam o limite, máximo 90,027 ms. Não há confirmação de equivalência visual pixel a pixel.

Artefatos: `artifacts/perf-stress-*-20261124-summary.json` e logs correspondentes. A meta exige investigar a codificação serial do wgpu, gargalos de streaming e dependências entre passes; nenhuma rodada acima aprovou o gate.

## Rodada seguinte: pressão de memória e pipelines

| Experimento | Mediana FPS | Mínimo FPS | Pico de memória privada (GiB) |
| --- | ---: | ---: | ---: |
| Caminho rápido de bind groups (revertido) | 31,99 | 12,99 | 8,26 |
| Remoção de pipelines não utilizados (revertida) | 29,96 | 18,98 | 8,27 |
| Aprendizado, pré-compilação integral | 42,00 | 31,97 | 7,83 |
| Histórico seletivo | 48,90 | 39,95 | 6,84 |
| Variantes relacionadas (revertido) | 45,95 | 13,74 | 8,12 |
| Histórico seletivo ampliado | 30,97 | 20,98 | 8,18 |

O catálogo contém mais de 22 mil receitas, mas apenas 897 foram usadas na primeira sessão de aprendizado. A seleção inicial reduziu a memória privada, porém introduziu compilações sob demanda; em sessões mais pesadas, milhares de variantes adicionais foram solicitadas. Portanto, o modo seletivo não é padrão. `WGR_HOT_PIPELINES=learn` registra uso com pré-compilação integral; `WGR_HOT_PIPELINES=1` é experimental. Por padrão, não há gravação de histórico durante o gameplay, e a deduplicação de receitas evita trabalho redundante no carregamento. Os resultados variam conforme a cena, e nenhum teste atingiu 60 FPS mínimos; os logs não comprovam paridade visual.
