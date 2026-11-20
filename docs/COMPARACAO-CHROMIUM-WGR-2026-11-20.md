# GTA V Web Port: Chromium × WGR nativo

**Execução real no Windows, 20/11/2026.** A investigação utiliza o WGR e a
mesma referência do jogo `8b0b5899ed`. O objetivo é identificar
divergências do runtime nativo, **não** incluir um navegador no aplicativo.

## A referência correta

O endereço `https://fiveplay.lol/?mode=sandbox` agora abre uma página de
entrada. Seu botão **Play Now on Version 2** encaminha para
`https://v2.fiveplay.lol/`, que carrega `/launcher.html`. Em seguida,
o botão Play abre `/game.html?mode=sandbox&cfg=1`.

Usamos **Microsoft Edge 157 (Chromium)**, instalado no computador de teste,
com um perfil exclusivo em `artifacts/browser-game-reference-edge/isolated-profile`.
O Chrome/Edge moderno exige que a depuração remota não utilize o perfil padrão
do usuário. Nenhum navegador, extensão, conta pessoal ou repositório de
terceiros foi modificado. O Zen Browser é um fork do Firefox/Gecko; copiá-lo
ou compilá-lo não acrescentaria fidelidade a esta comparação com Chromium.

Os scripts que automatizam e documentam a referência são:

- `scripts/Compare-Browser-Gta.mjs`: inicializa Edge com perfil isolado,
  coleta amostras via Chrome DevTools Protocol e salva capturas;
- `scripts/Inspect-Browser-Gta.mjs`: inspeciona o endereço e as páginas,
  navega pelo link oficial e opcionalmente oculta o iframe de anúncio
  **somente para visualizar a tela do teste**, sem modificar gráficos;
- `scripts/Compare-Gta-Runtimes.mjs`: compara os relatórios da sessão
  Chromium e do WGR, com limites explícitos sobre dados não equivalentes.

Exemplo de reprodução:

```powershell
node scripts/Compare-Browser-Gta.mjs --seconds 180 --play --url https://v2.fiveplay.lol/launcher.html --output artifacts/browser-game-reference-edge
# Em outro terminal, usando a porta impressa pelo comando acima:
node scripts/Inspect-Browser-Gta.mjs <PORTA> --dismiss-overlays
```

**Referência visual real:** `artifacts/browser-game-reference-edge/browser-120s.jpg`.
No Edge, personagem, roupa, superfície do solo, objetos e sombras aparecem
renderizados. No WGR, as capturas
`artifacts/gta-black-regression-20261120.png` e
`artifacts/gta-lighting-parity-20261120.png` mostram personagem preto,
superfície cinza sem detalhes e geometria incompleta. Os locais/câmeras são
diferentes: a comparação é qualitativa, **não** pixel a pixel.

## Principais diferenças identificadas

| Aspecto | WebGPU no Chromium | Rust/wgpu no WGR |
| --- | --- | --- |
| Compilação de pipelines | `wgpu_worker.js` rastreia jobs de iluminação (`deferred_lighting`, `tiled_lighting`, `emissive`) e espera uma conclusão para tentar o desenho novamente | O `get_pipeline` original esperava apenas **8 ms compartilhados por frame**; após o limite, o desenho podia ser descartado |
| Geometria | Cena do navegador com superfícies e personagem texturizados | Cena nativa com terreno cinza e personagem preto apesar de múltiplos desenhos por frame |
| Texturas de personagem | Personagem texturizado visualmente | Sondas de shader mostram SRVs válidos, mapas BC1/BC3 de 256–1024 px e uploads presentes; portanto não basta procurar arquivos ausentes |
| GPU | RTX 4060 Laptop detectada pelo protocolo do Edge | `wgpu` já usa GPU nativa, através de Vulkan ou DX12, sem WebView/Chromium |
| Erros de validação | O protocolo do navegador não fornece contagem diretamente equivalente | Última sessão nativa mostra **0 erros de validação** mesmo com a imagem incorreta |
| FPS/RAM | Medições por aba do Chrome não cobrem toda a memória do worker/Wasm | FPS nativo mede submissões; heap linear Wasm relatado como 3072 MiB no runtime |
| Mouse horizontal | Referência pode processar entrada pela pilha de controles do navegador | A entrada dX original não fazia girar a câmera; a rota `SetControlValueNextFrame(0,1,...)` foi testada e mudou o yaw de 131,35° para 58,49° em 120 frames |

O código de referência WebGPU foi inspecionado a partir do endpoint do
próprio port, sem modificação ou redistribuição dentro do runtime:
`https://v2.fiveplay.lol/b/8b0b5899ed/wgpu_worker.js`.
Arquivo local de estudo:
`artifacts/browser-game-reference-edge/reference-wgpu-worker.js`.
A identificação de build `8b0b5899ed` coincide com o
`BUILD` em `vendor/muguet/src/remote.rs` e com
`ports/fiveplay-gta5.port.json`.

## Implementação e estado

1. O WGR adicionou um caminho explícito de controle horizontal para o GTA
   (`WGR_MOUSE_CONTROL_LOOK`), com valor analógico assinado e limitado.
   A fonte dX da memória Wasm não foi removida. Existe opt-out com
   `WGR_MOUSE_CONTROL_LOOK=0`. A câmera girou no teste de comando nativo,
   mas **o hardware real ainda deve ser validado em jogo**.
2. A leitura de relatórios do launcher foi corrigida para aceitar mensagens
   de material acima de 8 KiB, até 64 KiB, descartando linhas maiores com
   segurança. Isso revelou as texturas e constantes usadas nos shaders
   de personagens, e dois testes de unidade passaram.
3. A espera de pipelines críticos de iluminação foi implementada com limite
   de 1200 ms apenas na primeira ocorrência de cada estado, em no máximo
   24 estados. O resultado da captura **ainda mostra problemas graves**;
   logo essa hipótese não explica tudo.
4. Está disponível a seleção experimental de backend
   `WGR_GPU_BACKEND=dx12` ou `WGR_GPU_BACKEND=vulkan`, para comparar
   a imagem em ambas as APIs. Sem a variável, preserva o comportamento
   original de seleção automática.
5. Foram acrescidos logs de destinos de renderização, máscaras de escrita,
   shaders efetivamente usados e referências de texturas. Uma unidade
   do tipo `ped` escreveu em quatro alvos BGRA8 com diferentes máscaras.
   Isso restringe a próxima investigação à equivalência dos passes,
   iluminação, profundidade/oclusão e interpretação de GBuffer.

## Limites importantes dos números

O DevTools conectado à **aba** mostrou ~2,3 MiB de tráfego e ~12 MiB de JS heap,
mas o próprio progresso do GTA relatou ~277,9 MiB de dados do jogo no worker.
Isso **não** é uma contradição: `Network` e `Performance.getMetrics`
do target da aba não incluem integralmente o worker, a memória linear do
WebAssembly, a memória da GPU e o armazenamento externo. Jamais usar esses
12 MiB como referência de RAM total para o WGR.

As mensagens do worker também aparecem no canal de erros do CDP mesmo
quando são linhas de log informativas; contar todas como falhas JavaScript
seria enganoso. A primeira sessão tinha **zero exceções JS** capturadas.

## Critérios antes de declarar o GTA corrigido

O teste automático só pode aprovar ABI, inicialização, mundo reportado,
erros de validação, desenho submetido, leitura de dados e arquivos de saída.
É necessário validar separadamente: personagem realmente texturizado,
terreno/modelos presentes, sombras, posição da câmera e **movimento
horizontal e vertical do mouse físico**. Uma taxa de 100 FPS não confirma
qualidade. Não declarar o jogo completamente funcional até esses
requisitos estarem satisfeitos.

## Diagnóstico de DX12 (20/11)

Além da referência em Chromium, duas execuções nativas foram feitas com
`WGR_GPU_BACKEND=dx12`, mantendo o launcher sem navegador:

- **Antes do fallback**: vários shaders de vértices não compilavam devido a
  `enable clip_distances`, indisponível no dispositivo DX12 selecionado.
- **Depois do fallback**: a transformação dos atributos de recorte foi
  adicionada ao Rust e os 18 testes unitários do Muguet passaram. Os erros
  de criação daqueles shaders desapareceram.
- **Ainda falhou no DX12**: a GPU voltou a registrar erros em shaders de
  computação de iluminação (`tiled_lighting::DepthDownsampleCS_Default`,
  `avgLuminanceCompute::LuminanceDownsample*` e `adaptiveDof::ReductionCS`).
  O compilador FXC diagnosticou condição de corrida em escrita de recurso
  compartilhado, tornando esses pipelines inválidos. A captura de 5 segundos
  depois do evento de mundo ainda mostrava a apresentação inicial;
  não há confirmação visual de que a cena tenha passado a funcionar.
- **Consequência**: `WGR_GPU_BACKEND` é ferramenta de A/B. Sem essa
  configuração, a seleção anterior do adaptador permanece. Não ativar
  DirectX 12 indiscriminadamente nem confundir eliminação dos erros de
  `clip_distances` com renderização correta.

Evidências:

- `artifacts/gta-dx12-parity-20261120.json/.png` — DX12 antes da correção;
- `artifacts/gta-dx12-clipcompat-20261120.json/.png` — DX12 após a correção;
- `artifacts/gta-browser-native-comparison-20261120.json/.md` — comparação
  estruturada do Chromium com a execução Vulkan/default anterior;
- `artifacts/browser-game-reference-edge/reference-wgpu-worker.js` —
  fonte do port consultada para entender a equivalência do fallback.

A sessão de navegador é uma **referência funcional**, não prova de paridade.
O personagem preto, parte dos modelos e terreno cinza **continuam
sem correção visual verificada**. Não usar as capturas DX12 como evidência
de melhoria até que o mundo 3D seja efetivamente apresentado.

Referências de instrumentação:

- https://chromedevtools.github.io/devtools-protocol/tot/Tracing/
- https://chromedevtools.github.io/devtools-protocol/tot/Performance/
- https://developer.chrome.com/blog/remote-debugging-port
- https://chromium.googlesource.com/chromium/src/+/main/docs/memory/tools.md
