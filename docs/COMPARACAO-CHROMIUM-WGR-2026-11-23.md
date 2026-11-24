# Benchmark local: Chromium × WGR — 23/11/2026

Windows, RTX 4060 Laptop, 20 processadores lógicos, 16 GiB de RAM.
Port FivePlay GTA V, modo sandbox. Comparação sequencial, cache aquecido,
1920 × 1200 confirmado no canvas Chromium e no log do backbuffer nativo.
Sem redução de escala ou alterações de qualidade gráfica nesta rodada.

| Métrica | Edge 158 | WGR antes | WGR final |
| --- | ---: | ---: | ---: |
| FPS mediano | 42 | 45,22 | 65,52 |
| FPS mínimo amostrado | 40 | 27,98 | 58,73 |
| FPS máximo amostrado | 48 | 55,53 | 67,32 |
| Percentil 10 de FPS | 40 | 32,93 | 61,69 |
| Mundo pronto (s) | 39 | 43,62 | 21,33 |
| Primeiro frame nativo (s) | — | 19,22 | 3,29 |
| RAM residente de pico (GiB) | 5,34 | 5,42 | 4,97 |
| Memória privada de pico (GiB) | 9,97 | 7,86 | 7,07 |
| VRAM dedicada de pico (GiB) | 3,54 | 2,29 | 2,27 |
| CPU média aproximada (% dos 20 lógicos) | 19,0 | 8,0 | 11,0 |

## Alterações

- Índice SQLite para referências por hash; consultas preparadas reutilizadas.
- Lotes LRU de 512 entradas, sem remover a verificação SHA-256.
- Cache quente de 256 MiB por padrão; override existente preservado.
- Cache de bind groups usa último frame de acesso; elimina grupos inativos
  periodicamente e preserva identidade/geração dos recursos.
- Benchmark Chromium mede o FPS do próprio jogo, aceita viewport explícito
  e reutiliza apenas o perfil isolado indicado.

## Evidências locais

- `artifacts/perf-compare-native-20261123-summary.json`: baseline nativo.
- `artifacts/perf-cache-index-20261123-summary.json`: primeiro ajuste (51,73 FPS).
- `artifacts/perf-cache-256-20261123-summary.json`: cache maior (60,65 FPS).
- `artifacts/perf-final-default-20261123-summary.json`: repetição (50,72 FPS).
- `artifacts/perf-bind-prune-20261123-summary.json`: build final (65,52 FPS).
- `artifacts/browser-1920x1200-20261123/reference.json` e `resources.json`: Edge.

## Limites

Janela de aproximadamente 30 s após o evento de mundo; 25 amostras nativas e
26 no Chromium, excluindo os primeiros quatro segundos. FPS reportado pelo
port, não por apresentação física. Resolução efetiva igual, mas estado exato
do mundo, câmera e qualidade pixel a pixel não foram equalizados. CPU e RAM
dependem também de cache e pressão de memória do sistema; os pontos de partida
dos relógios de carregamento diferem. Uso percentual da GPU não é equivalente
entre os dois instrumentos. Não foi medido tempo individual de cada asset.
O build final reportou zero erros de validação wgpu, mas isso não comprova
paridade visual. A meta de 60 FPS mínimos e 100 FPS máximos ainda não foi atingida.
