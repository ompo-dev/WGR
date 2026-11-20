# WGR / GTA V — desempenho nativo, 20/11/2026

## Objetivo e limites

Melhorar os FPS e a estabilidade do GTA V no runtime Rust/Wasmtime/wgpu,
preservando a resolução interna original, geometria, texturas, shaders,
iluminação, efeitos, distância de visão e oclusão. A meta de aproximadamente
100 FPS está **em aberto**; não foi comprovada em cenários densos.

Os números abaixo são reportados pelo runtime durante sessões reais e **não**
equivalem a um teste determinístico de apresentação física no monitor. Os
cenários, deslocamentos, clima, conteúdo em cache e carga de streaming variam
entre sessões; portanto, não se deve inferir ganho percentual causal apenas
comparando medianas de duas sessões diferentes.

## Ambiente e resultados registrados

- Windows, NVIDIA RTX 4060, backend Vulkan; o DX12 apresentou pipelines de
  compute inválidos e não foi aprovado para uso padrão.
- Backbuffer interno de **1920 × 1200** em todas as medições desta seção.
- Frame de jogo com aproximadamente 2.587 desenhos e 180 passes: **28,8 ms**,
  incluindo **9,64 ms em draw** e **10,65 ms em readback**, com **9,91 ms em
  `Queue::submit`**. Draw, readback e submit possuem tempos inclusivos que
  se sobrepõem; não se deve somá-los como custos independentes.
- Em amostras de cenas densas, cerca de 2 leituras GPU→CPU e 3,1 submissões
  ao driver por frame; uso da RTX 4060 observado pontualmente em 24–27%.

| Sessão | Tempo até o mundo | P10 / mediana reportados | Observação |
| --- | ---: | ---: | --- |
| `fps-vulkan-smallvec-no-texture-diag-20261120` | 28,509 s | 37,78 / 42,73 FPS | 217 amostras após 75 s; sem diagnósticos de textura; cache de constantes desligado |
| `fps-vulkan-opcode-profile-20261120` | 44,971 s | 22,84 / 35,72 FPS | 73 amostras após 75 s; cena mais pesada; custos detalhados por opcode |
| `fps-vulkan-readback-ordered-buffers-20261120` | 23,309 s | 4,95 / 28,98 FPS | 101 amostras após 60 s; **regressão** no experimento de readback adiado |

Os relatórios `gta.json`, `summary.json`, `engine.log` e capturas, quando
disponíveis, estão sob `artifacts/<nome da sessão>/`.

## O que foi mantido

1. `SmallVec` para listas temporárias de recursos e chaves de bind groups na
   montagem de cada draw, reduzindo alocações no caminho quente sem alterar
   conteúdo, ordenação ou identidade das chaves persistentes.
2. Instrumentação ativável com `MUGUET_PERF_DIAGNOSTICS=1`, medindo tempo de
   submissão, espera, criação de bind groups, readbacks e opcodes de draw,
   dispatch e upload.
3. Resolução de renderização padrão mantida em 100%. Nenhum ajuste de
   render scale, iluminação ou qualidade foi usado para obter os resultados.

## Experimentos descartados

O adiamento de readbacks para agrupar submissões foi testado com cópias
ordenadas e barreiras de segurança. Apesar de reduzir submissões diretamente
atribuídas ao readback, provocou janelas com 3–15 FPS e longas esperas por
comandos sob carga. **A implementação experimental foi retirada** e o caminho
anterior, de submissão imediata e mapeamento assíncrono, foi preservado.

O cache de constantes por versão de buffer apresentou muitos acertos e
economizou cópias internas, mas testes com cenas variáveis não comprovaram
melhora consistente de FPS; não é suficiente atribuir-lhe um ganho percentual.

## Reprodução e próximos critérios

No PowerShell, executando do diretório do projeto, os testes de Rust usam:

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo test --release --offline --locked --manifest-path vendor/muguet/Cargo.toml
cargo clippy --release --offline --locked --manifest-path vendor/muguet/Cargo.toml --all-targets -- -D clippy::correctness
cargo build --release --offline --locked --manifest-path vendor/muguet/Cargo.toml
```

Para perfilar sem os logs pesados de textura: backend Vulkan,
`MUGUET_PERF_DIAGNOSTICS=1`, `MUGUET_UNIFORM_CACHE=0`, sem `MUGUET_RES`
e sem `WGR_RENDER_SCALE`; usar o smoke real do GTA sem `--gta-logs`.

As próximas alterações precisam ser aceitas somente quando (a) mantiverem
paridade visual, (b) passarem nos testes e no smoke do player, (c) reduzirem
tempo de frame P95/P99 e travamentos em uma **cena repetível**, e (d) não
degradarem combate, explosões, veículos ou voos. O custo de `Queue::submit`,
dos muitos passes e da produção de comandos pela engine/Wasmtime permanece
como foco principal; não foi solucionado neste ciclo.

## Validação final da retomada

- Foram removidos integralmente o agendamento de readbacks diferidos e as
  barreiras experimentais após a regressão registrada. O readback voltou a
  ser submetido e mapeado no ponto previsto pela implementação estável.
- As chaves temporárias de bind groups e a chave de 37 palavras de identificação
  do render pass passaram a usar `SmallVec` inline. A mudança é estrutural;
  não muda o conjunto de render targets nem configurações gráficas.
- `cargo fmt --check`, os **32 testes** do runtime, `cargo clippy` com
  `-D clippy::correctness` e o build release foram aprovados (os avisos
  não bloqueantes de estilo continuam registrados no código).
- GTA final, sem flags de depuração, em `artifacts/gta-vulkan-inline-passkey-20261120/`:
  mundo detectado aos **30,351 s**, captura `gta.png` presente, imagem interna
  em **1920 × 1200** e inicialização encerrada. A captura mostra uma região
  urbana/industrial à noite; essa diferença de horário e cena impede comparação
  visual pixel a pixel com capturas diurnas de execuções anteriores. O smoke
  **não** confirmou controles, exploração livre nem desempenho sustentado.
- Player smoke em `artifacts/smoke-summary.json`: **3/3 execuções aprovadas**,
  primeiro frame mediano **104 ms** (91–200 ms), save restaurado. Os números
  são da fixture local, **não** do GTA nem de um navegador externo.
- A configuração segura publicada permanece em `wgr-rage.exe`, com backup da
  versão anterior em `artifacts/wgr-rage.before-inline-passkey-20261120.exe`.
  Nenhuma promessa de 100 FPS sustentados é suportada pelos testes atuais.
