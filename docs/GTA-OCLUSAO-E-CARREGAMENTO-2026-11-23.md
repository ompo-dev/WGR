# GTA V no WGR — diagnóstico de oclusão e integridade visual

## Contexto

Na build `333b8b9`, uma execução apresentou chão cinza e construções ausentes após o evento `world_confirmed`, enquanto outras execuções mostraram o cenário completo. A meta desta rodada é preservar a renderização correta **sem reduzir** a resolução, a distância de visão, os efeitos ou a qualidade de texturas. A entrada no mundo **não** significa que todos os assets de uma região foram carregados.

## Procedimento

1. Teste nativo no backend Vulkan, usando a mesma build base e dois valores diagnósticos: `MUGUET_DIAG_OCCLUSION_VISIBLE=0` (resultado de GPU normalmente aplicado) e `=1` (todos os resultados forçados a visíveis). O segundo valor é apenas diagnóstico, nunca configuração de desempenho.
2. Capturas do **HWND do próprio WGR** e relatórios em `artifacts/occlusion-baseline-20261123/` e `artifacts/occlusion-force-visible-20261123/`.
3. Inspeção de `vendor/muguet/src/gpu.rs`, especialmente `begin_occlusion_in_pass`, `end_query`, `record_resolves`, `process_maps`, `flush` e a conclusão assíncrona de queries.
4. Correção conservadora e nova execução com o comportamento normal, em `artifacts/occlusion-unknown-safe-20261123/`.

As cenas e os pontos de spawn entre execuções não são idênticos. Portanto, os resultados de FPS entre essas sessões **não são um teste A/B causal** e nenhuma captura isolada prova que o bug visual intermitente desapareceu.

## Problema comprovado no código

- Antes da correção, uma consulta de oclusão sem `slots` era tratada como zero amostras no `record_resolves`, mesmo que **nenhuma amostra GPU tivesse sido produzida**. O resultado era publicado imediatamente no endereço compartilhado do guest, antes de respeitar a conclusão de comandos anteriores da fila.
- Se os 2.048 slots do conjunto de queries esgotassem, `begin_occlusion_in_pass` não registrava a parte ausente como consulta incompleta. Isso também podia transformar dados desconhecidos em zero e esconder geometria.
- Uma consulta válida com zero pixels passando no depth test deve continuar informando zero. Não é correto desativar globalmente a oclusão: isso elevaria o trabalho de GPU e poderia reduzir a fluidez.

## Correções no motor nativo

- `occlusion_guest_result(Some(0), false)` retorna zero. Com amostras positivas, retorna visível. Com `None` (ausência de resultado), retorna visível por segurança (*fail-open*).
- O fim de uma consulta sem slot é publicado via `pending_events` **após a fila de GPU**, em vez de gravar imediatamente zero na memória guest.
- Ao esgotar slots, a consulta ativa recebe `skipped_draw=true`, para não aceitar um resultado parcial como se estivesse completo.
- `flush()` conclui também eventos que não possuem encoder de comandos, usando callback da fila; evita deixar queries sem resultado indefinidamente.
- Contador `exhausted` acrescentado à instrumentação existente, além de `visible`, `zero`, `empty` e `incomplete`.
- Teste unitário novo verifica que `Some(0)` continua ocultando e `None` nunca é considerado oculto.

## Evidências obtidas

| Execução | Mundo detectado | FPS P10 | FPS mediano | Observações |
| --- | ---: | ---: | ---: | --- |
| Código anterior, resultado normal | 36,23 s | 52,68 | 63,55 | 72 amostras, captura da região sob viaduto completa |
| Código anterior, tudo visível (diagnóstico) | 34,67 s | 41,50 | 57,57 | 240 amostras; cena diferente; maior carga de draw |
| Código corrigido, resultado normal | 66,02 s | 63,53 | 71,58 | 240 amostras; cena diferente, com paisagem, prédios e vegetação nas capturas |
| Código corrigido, segunda execução sem flags de diagnóstico | 125,29 s | 51,67 | 63,64 | 238 amostras; praça urbana com piso, edifícios, veículos, personagem e vegetação presentes |

No teste anterior com oclusão normal, o log chegou a `visible=5`, `zero=24817`, `empty=0`. Com a visualização forçada, chegou a mais de 23 mil eventos `empty`. Na execução corrigida com oclusão normal, o log registrou `empty=9350`, `exhausted=0` e `incomplete=0`. **Ou seja, consultas sem slot ocorrem mesmo sem esgotar o conjunto de slots**; elas não devem ser presumidas invisíveis. Esses contadores acumulados não medem diretamente quantos meshes foram recuperados.

A validação do runtime nativo passou em **36 testes**, incluindo a regressão nova; `clippy -D clippy::correctness` e build release passaram. A métrica GPU ainda não mede memória física dedicada via Windows; o tamanho de heap WASM também não é uso exclusivo de RAM.

### Localizações das amostras

O `world_report.payload.position` permite verificar onde cada execução começou, embora o smoke não controle ainda câmera, horário, tráfego ou trajetória. A execução anterior com visibilidade forçada e a primeira corrigida começaram aproximadamente em `(-841.29, 158.08, 67.2)`, mas usaram configurações gráficas diferentes. A segunda corrida corrigida começou em `(94.95, -742.96, 45.75)`; a captura dessa região mostrou a praça completa. A captura cinza da rodada anterior (`gta-final-verified-20261123`) começou em `(-59.74, -1110.62, 26.44)`, **posição diferente**. Não é possível declarar esse cenário específico corrigido a partir das imagens atuais.

O segundo teste também confirmou a necessidade de otimização independente do *loading*: **125,29 segundos até o evento de mundo**, `disk_lookup_ms` acumulado de aproximadamente **613 mil ms** entre os leitores paralelos, mais de 224 mil hints coalescidos e mensagem da engine de espera por operações de streaming. Esses contadores são cumulativos em múltiplas threads, não se somam diretamente ao tempo de parede. O cenário mostrou geometria correta quando capturado, apesar do carregamento lento.

## Critérios pendentes

- Reproduzir o **mesmo ponto de spawn/posição e trajetória**, com cache frio e quente, em ambas as builds. As execuções desta rodada têm regiões diferentes.
- Fazer captura e verificação de geometria nos primeiros 30, 60 e 120 segundos após `world_confirmed`. Não aceitar como sucesso apenas FPS alto ou o primeiro frame.
- Investigar no carregador as rajadas de `hints_source_coalesced` e latências de `disk_lookup_ms` na sessão de chão cinza. Esses indicadores podem representar um segundo problema, distinto da oclusão.
- Verificar shader inválido `cascadeshadows_processing::PS_DownSampleShadow` (escrita de profundidade sem depth attachment), que continua como problema separado de pipeline de sombra, não evidência de perda de toda a geometria.
- Repetir sob voo, explosões, movimento rápido e leitura de assets em tempo real antes de afirmar 100 FPS sustentados ou jogabilidade integral.

Os saves, os perfis e a interface MomorRust não foram alterados nesta rodada.
