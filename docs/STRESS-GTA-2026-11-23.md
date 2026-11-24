# GTA V — benchmark de stress 23/11/2026

Sessão manual do usuário (20:32): mínimo de 18,99 FPS em jogo, até 5.182
draws/frame, 341 passes/frame, 608 novos pipelines e leituras de assets
de até 398 ms. Os logs originais foram preservados em `artifacts/stress-user-20261123-*.log`.

## Alterações mantidas

- Reutilização de pipeline layouts por identidade dos bind group layouts;
  cache de driver em memória quando suportado pelo adaptador, sem importar
  binários externos; `WGR_DRIVER_PIPELINE_CACHE=0` permite desativar.
- Evita invalidar o pipeline por reenvio de render targets idênticos.
- Grava receitas de pipelines em lotes, não uma abertura de arquivo por receita.
- Agrupa até oito readbacks consecutivos já publicados pelo guest, sem
  alterar a ordem das cópias; submete antes de outro opcode ou de ring ocioso.
- Workload local opt-in `WGR_TEST_STRESS=1`: seis jatos do modelo Lazer e
  doze chamadas de explosão, com remoção dos veículos de teste no final.
  Não é acionado em lançamentos normais.

## Resultados reais

1920×1200, GPU nativa; aproximadamente 30 segundos após evento de mundo.

| Rodada | Jatos | Explosões | Mediana FPS | Mínimo FPS | Máximo FPS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Pipeline/layout (diagnóstico) | 1 | 12 | 55,71 | 24,39 | 69,18 |
| Readbacks agrupados (diagnóstico) | 6 | 12 | 42,75 | 27,88 | 54,67 |
| Uniform shadow experimental (rejeitado) | 6 | 12 | 26,81 | 21,85 | 62,50 |
| Release mantida (sem diagnóstico) | 6 | 12 | 50,67 | 18,90 | 67,27 |

O cenário e a população não são determinísticos entre execuções. Não atribuir
causalidade a diferenças de FPS entre essas rodadas. A tentativa de alteração
de uniform shadow foi revertida e o executável anterior foi restaurado.
Os logs e resumos de cada execução estão em `artifacts/stress-*-20261123-*.log`
e `artifacts/perf-stress-*-20261123-summary.json`.

Reprodução: em PowerShell, com o WGR fechado:

```powershell
$env:WGR_TEST_STRESS='1'
try { & scripts/Benchmark-Gta.ps1 -Name stress-repeticao -Fps 0 }
finally { Remove-Item Env:WGR_TEST_STRESS -ErrorAction SilentlyContinue }
```

## Pendências

O problema de FPS mínimo não está resolvido. A rodada mais pesada chegou a
5.250 draws/frame, 395 passes/frame e 12,28 ms/frame de submissão, além de
10,90 ms/frame no caminho de draw (medidas de CPU, não GPU timestamps).
Agrupar readbacks adjacentes diminuiu pouco o número de submits: a maioria
não é adjacente. O próximo alvo é a preparação/validação e submissão de
comandos, com profiling por thread e medição do tempo real de GPU.
Nenhum novo benchmark Chromium com esta carga foi realizado; paridade visual
e meta de 60 FPS mínimos/100 máximos continuam pendentes. Não reduzir qualidade.

## Segunda rodada: oclusão e streaming direto

- Agrupamento de consultas adjacentes de oclusão, com offsets individuais;
  reutilização do buffer de resolve e dos buffers de leitura já existentes.
- Estado de vertex buffers do render pass em array fixo, sem alocação por pass.
- Streaming de blocos verificados diretamente para a memória do guest, sem
  concatenar uma cópia intermediária. Leituras pequenas dispensam o Rayon
  interno; leituras grandes continuam divididas em partes concorrentes.
- `Benchmark-Gta.ps1` exige mínimo de 60 FPS amostrados quando
  `WGR_TEST_STRESS=1`, pelo menos 20 amostras e confirmação dos seis jatos,
  doze explosões e encerramento da carga. Grava o resumo mesmo ao reprovar.

| Rodada (6 jatos, 12 explosões) | Mediana FPS | P10 FPS | Mínimo FPS | Máximo FPS | RAM residente GiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Release anterior | 50,67 | 22,84 | 18,90 | 67,27 | 5,32 |
| Oclusão (diagnóstico) | 40,82 | 31,85 | 29,86 | 58,22 | 5,22 |
| Oclusão sem cache de uniforms | 39,72 | 29,83 | 28,81 | 58,62 | 5,27 |
| Streaming direto | 46,75 | 39,77 | 37,79 | 59,52 | 5,12 |
| Streaming direto (repetição) | 49,64 | 44,74 | 39,74 | 119,63 | 5,04 |

As duas últimas rodadas reprovaram corretamente no gate de 60 FPS, apesar
da melhora do mínimo observado. O pico de 119,63 FPS é uma amostra do port,
não desempenho sustentado nem apresentação física. Os cenários variam em
população, passes e número de desenhos, portanto não demonstram causalidade
isolada. A CPU média aproximada foi 12,1% antes e 12,8% na repetição; não
houve redução comprovada de CPU. Os 38 testes do runtime foram aprovados.
O próximo gargalo é o custo de codificação/submissão dos passes no wgpu-core:
separar tempos de `finish`, `submit` e espera real da GPU antes de mudanças
na arquitetura, preservando barreiras, validação e ordenação dos recursos.

## Terceira rodada: diagnóstico do wgpu-core

Todas as execuções confirmaram seis jatos e doze explosões, sem alterar
a resolução interna de 1920×1200. O gate de 60 FPS continuou reprovado.

| Experimento | Mediana | P10 | Mínimo | Máximo |
| --- | ---: | ---: | ---: | ---: |
| Evitar transição de init vazia (diagnóstico) | 47,75 | 38,74 | 37,80 | 55,65 |
| Evitar pre-pass vazio (diagnóstico) | 28,71 | 25,72 | 17,62 | 40,12 |
| Staging belt de uniforms (diagnóstico) | 48,58 | 40,75 | 37,73 | 64,13 |
| Evitar transição de init vazia (release) | 39,78 | 27,82 | 25,85 | 66,63 |

Os três experimentos foram revertidos por ausência de ganho consistente;
o executável da rodada de streaming direto foi restaurado. Logs e resumos
em `artifacts/stress-{init-barrier,empty-prepass,staging-belt,init-barrier-release}-20261123-*`
e `artifacts/perf-stress-*-20261123-summary.json`.

Telemetria preservada no fonte: `perf-encode` separa `finish`, `queue_submit`
e `uniform_upload`. Uma janela com 3.216 draws/frame e 300,7 passes/frame
registrou 8,68 ms em finish, 1,36 ms em submit e 1,32 ms em uniform upload;
draw no tradutor consumiu 7,02 ms. São tempos de CPU; não são timestamps
da GPU. Também houve uma leitura de asset de 10,24 s na rodada anterior.
O objetivo requer reduzir a codificação serial e as dependências de
readback sem eliminar trabalho gráfico necessário. Os cenários ainda
variam entre execuções e a equivalência visual não está comprovada.
