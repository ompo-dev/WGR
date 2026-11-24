# Comparação arquitetural e experimental: Chromium e WGR

## Referências de implementação

- V8: https://v8.dev/docs/wasm-compilation-pipeline e https://chromium.googlesource.com/v8/v8/+/main/docs/wasm/architecture.md — tiering e otimização de funções quentes; não assumir superioridade do Cranelift apenas por ser nativo.
- Chromium: https://chromium.googlesource.com/chromium/src/+/main/docs/security/research/graphics/webgpu_technical_report.md — serialização Dawn Wire, memória compartilhada, processo GPU.
- Dawn: https://dawn.googlesource.com/dawn/+/refs/heads/main/src/dawn/native/vulkan/CommandBufferVk.cpp — rastreamento de descriptors, barreiras e codificação nativa.
- Worker local de referência `artifacts/browser-game-reference-edge/reference-wgpu-worker.js`: flush imediato de readbacks, dirty chunks, packs recentes por shader, cache de pipelines por identidade/conteúdo e fast path de bindings por epochs/versões. O worker tem aproximações e flags opcionais; não presumir que todas estejam ativadas no teste anterior.

WGR já elimina o navegador e a serialização Dawn Wire, usando Wasmtime, ring compartilhado, tradutor D3D11/WebGPU e wgpu-core. Isso não elimina a codificação/validação tardia no `finish`, uploads, proxies, dependências de readbacks nem trabalho de CPU do motor. Paralelizar finalização por blocos preserva a ordem de submissão e usa a validação normal de wgpu. Não desligar validação nem reduzir qualidade para obter FPS.

## Fixture comparável

Mesmo WGSL (`scripts/fixtures/encoder-reference.wgsl`), 3.200 draws, alvo RGBA8 de 32×32, constantes dinâmicas e readback de pixel verde validado em cada execução; três aquecimentos e dez amostras. Executado sequencialmente na RTX 4060 Laptop. Medianas em ms, desde criação do encoder até readback; não é FPS de GTA, não é comparação de backend idêntico (backend Dawn não confirmado). O finish do browser retorna após enfileirar comandos, não após completar a codificação no processo GPU, portanto usar total para comparação.

| Passes | Chromium/Dawn | wgpu Vulkan | wgpu DX12 |
| ---: | ---: | ---: | ---: |
| 1 | 6,10 | 0,68 | 0,92 |
| 320 | 6,75 | 3,37 | 3,26 |
| 800 | 9,75 | 8,28 | 6,87 |

Segunda rodada Vulkan, mesma fixture, quatro workers e submissão ordenada:

| Passes | Serial | Divisão serial | Finish paralelo | Record + finish paralelos |
| ---: | ---: | ---: | ---: | ---: |
| 320 | 3,21 | 3,39 | 2,89 | 2,43 |
| 800 | 7,15 | 8,00 | 4,86 | 4,88 |

A fixture confirma que o backend nativo pode superar o Chromium em uma carga controlada e que a divisão só compensa com paralelismo. Não prova paridade de cenas GTA. DX12 não foi adotado no jogo: incompatibilidades de shaders já observadas em sessões anteriores.

## Gameplay: seis jatos e doze explosões, 1920×1200

| Rodada | FPS mediano | FPS mínimo | P10 | Memória privada GiB |
| --- | ---: | ---: | ---: | ---: |
| Otimizações de referência (serial) | 36,75 | 25,83 | 30,84 | 7,75 |
| Finish paralelo (diagnóstico) | 58,49 | 46,72 | 49,68 | 7,17 |
| Finish paralelo (sem diagnóstico) | 50,68 | 34,82 | 36,80 | 7,59 |
| Finish antecipado (revertido) | 52,71 | 14,90 | 19,87 | 7,88 |

O workload não é determinístico em população/clima e número de passes. Nenhuma rodada aprovou 60 FPS mínimos ou frame pacing. As duas rodadas paralelas registraram zero erros de validação; isso não prova igualdade visual. CPU média aproximada de 13,8–14,3% nas rodadas paralelas; não demonstramos redução de CPU. Na segunda houve 5.582 draws/frame e uma leitura de asset de 5,2 s. A primeira teve ~3.733 draws e 312,5 passes/frame com finish de 5,17 ms, versus 8,68 ms observado em outra sessão serial (não é A/B de mesma cena). A finalização antecipada foi removida.

## Código e reprodução

- Cache de scans negativos de proxies invalidado por mudanças de SRV, criação/destruição/reuso de texturas/views e reset de estado.
- Constantes: copia somente a parte preenchida e zera o restante de cada membro contíguo; só o slot ativo pode ter uploads pendentes antes da rotação de frame.
- Pipeline: examina apenas streams do layout e copia a assinatura de vértice no miss.
- `WGR_PARALLEL_FINISH=1`: experimental, divide o encoder em blocos a cada 32 passes, com até oito blocos por submissão; quatro workers finalizam em paralelo, mantendo a ordem. Desligado por padrão até comprovar ganho sustentado e paridade visual.
- `node scripts/Benchmark-Dawn.mjs artifacts/dawn-encoder-repeticao` (browser isolado, localhost). `cargo test --manifest-path vendor/muguet/Cargo.toml --test encoder_reference --release --locked --target-dir vendor/muguet/target -- --ignored --nocapture` (GPU física; não executar simultaneamente com outros benchmarks). `WGR_GPU_BACKEND=dx12` seleciona DX12 na fixture.
- Logs/resultados: `artifacts/dawn-encoder-20261124/`, `artifacts/perf-stress-{browser-parity,parallel-finish,parallel-release,stream-finish}-20261124-summary.json` e logs correspondentes.

Pendências: trace comparável de jogo pesado no browser, cena determinística, reduzir pressão do streaming e traduções de bindings, validar paridade visual e consumo. O requisito de 60 FPS não foi cumprido e não pode ser inferido a partir da fixture.

## Rodada adicional: bindings e uploads

| Experimento | FPS mediano | FPS mínimo | Maior intervalo de apresentação |
| --- | ---: | ---: | ---: |
| Upload direto + lock de janela I/O (revertidos) | 25,87 | 14,91 | 108,52 ms |
| Cache local de bindings por shader + finish paralelo | 56,59 | 30,81 | 64,36 ms |
| Comparação de conteúdo de constantes (revertida) | 50,65 | 31,66 | 96,72 ms |
| Cache recente de constantes + scratch de uploads (revertidos) | 30,84 | 11,92 | 104,80 ms |

O cache local experimental `WGR_LOCAL_BIND_CACHE=1` referencia a chave do cache global, não mantém objetos GPU adicionais, valida IDs/gerações, máscaras e ausência de clears/ASTC pendentes; proxies, armazenamento e shaders especiais seguem o caminho completo. Na rodada de conteúdo, 2.455.425 de 3.573.146 acertos de bindings foram locais em uma janela, mas nenhuma das 2.585.904 escritas de shadow nessa janela era idêntica. O cache recente não compensou picos de upload de 22,61 ms/frame. Nenhum teste passou o gate de 60 FPS. Cenas não são idênticas entre execuções.

Fixture adicional `large_uniform_upload_reference`: vinte amostras por tamanho, com readback conferindo o valor enviado. Aumentar o pool wgpu-core de 4 para 8 MiB por buffer mantendo 64 MiB totais não trouxe ganho consistente. Mediana total (ms) antes/depois: 2 MiB 0,276/0,292; 5 MiB 0,595/0,663; 7 MiB 0,795/0,997. Limite restaurado. Resultados em `artifacts/upload-pool-{before,after}-20261124.txt`. Os experimentos não demonstram que o staging seja a causa isolada dos picos em gameplay.

Próximo diagnóstico estrutural necessário: comparar tempo e memória em uma sequência de comandos gráficos idêntica com carga de assets e pipelines real, separar tempo de alocação, cópia e mapeamento dentro do staging, e verificar contenção/paginação do sistema. A meta de 60 FPS sustentados e a paridade visual permanecem abertas.

## Rodada adicional: compartilhamento de uniformes e máscaras de vértices

| Experimento | FPS mediano | FPS mínimo amostrado | Pior intervalo | Memória privada de pico |
| --- | ---: | ---: | ---: | ---: |
| Layouts compartilhados, 8 MiB | 51,59 | 36,80 | 122,09 ms | 7,40 GiB |
| Máscaras de vértices, 8 MiB | 53,67 | 50,64 | 75,02 ms | 7,14 GiB |
| Repetição sem diagnóstico | 29,82 | 26,80 | 79,56 ms | 7,35 GiB |
| Pré-compilação guiada por shaders | 26,81 | 20,88 | 123,55 ms | 7,58 GiB |
| Uniformes de 4 MiB | 52,66 | 46,67 | 47,33 ms | 6,99 GiB |
| Uniformes de 2 MiB | 43,76 | 32,79 | 61,86 ms | 7,20 GiB |

Todas as seis rodadas completaram seis jatos e doze explosões, sem redução deliberada de resolução; todas reprovaram o gate de desempenho. O FPS mínimo é de janelas amostradas, não um mínimo instantâneo. As cenas variaram em número de draws e passes, assim como a disponibilidade de RAM; não atribuir causalidade aos números sem replay idêntico. A repetição teve apenas 872 MiB de RAM disponível e pico global de 21.333 páginas lidas/s. A rodada de 4 MiB apresentou 1,48 ms/frame de uploads na janela pesada, 4.051 draws/frame e zero erros de validação registrados. Ainda houve 1.200 de 1.403 intervalos acima de 16,667 ms. Nenhuma paridade visual foi comprovada.

- `WGR_SHARED_UNIFORM_LAYOUTS=1`: compartilha alocações apenas entre layouts idênticos, com mesmos dados de origem e versões. Experimental, padrão desativado.
- Máscaras de slots ativos e modificados no wgpu-core: evita percorrer slots não modificados, preserva validação de buffers e limites.
- `WGR_SHADER_AWARE_WARMUP=1`: adia receitas até a declaração dos shaders, experimental; não demonstrou ganho sustentado.
- `WGR_UNIFORM_CHUNK_MIB=2|4|8`: tamanho configurável, padrão 8; 4 MiB foi mais promissor nessa rodada, mas não comprovado em replay equivalente.
- Validação: 41 testes do runtime, duas fixtures GPU com readback e três smoke tests locais aprovados. Artefatos `artifacts/perf-stress-{shared-layout,vertex-mask,vertex-mask-repeat,shader-aware,uniform-chunk4,uniform-chunk2}-20261124-summary.json` e logs correspondentes.
