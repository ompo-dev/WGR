# Servo no WGR — experimento de compatibilidade (20/11/2026)

## Decisão arquitetural

O WGR mantém duas rotas independentes:

1. **Nativa:** `WGR.exe` e `wgr-rage.exe`, ou o futuro `wgr-player` direto com Wasmtime, wgpu, winit e CPAL. Não substituímos o runtime GTA comprovado.
2. **Web:** Servo para ports cujo `index.html` depende de JS, DOM, Canvas, WebGL/WebGPU, IndexedDB ou APIs web. Primeira entrega: executável experimental `wgr-servo` que abre **servoshell como processo externo**, sem WebView2/Chromium. Isso **não é embedding** no GPUI nem uma migração do launcher.

Não incluir `webrender`, `surfman` e `mozjs` isoladamente como três dependências do WGR: esses componentes já são integrados pelo Servo. O WebRender compõe conteúdo web, não substitui o protocolo GPU de um port RAGE; Surfman gerencia contextos/superfícies OpenGL; MozJS executa JavaScript/Wasm, mas não fornece DOM ou APIs web sozinho.

Referências locais **somente leitura**:

- `C:\Users\maico\OneDrive\Documents\GitHub\webrender` — revisão `e1c924eba`, 07/10/2025
- `C:\Users\maico\OneDrive\Documents\GitHub\surfman` — revisão `f38883b`, 19/11/2026
- `C:\Users\maico\OneDrive\Documents\GitHub\mozjs` — revisão `37f1c8fc8`, 15/11/2026
- Servo: https://github.com/servo/servo
- Referência de embedding: https://book.servo.org/embedding/overview.html
- Recursos experimentais: https://book.servo.org/design-documentation/experimental-features.html

## Executar

Compile `wgr-servo` com `cargo build -p wgr-servo --release --locked`.
Use um `servoshell.exe` oficial ou compilado do Servo, informado explicitamente:

```powershell
.\target\release\wgr-servo.exe --servo "C:\caminho\servoshell.exe" --probe
.\target\release\wgr-servo.exe --servo "C:\caminho\servoshell.exe" --bundle "C:\caminho\jogo"
```

O bundle deve conter `index.html` ou manifesto local compatível com `wgr-core::assets::inspect_bundle`.
O runner serve os arquivos por HTTP **somente em loopback**, reutilizando `wgr-http` (COOP/COEP/CORP, Range e validação de paths). Servo inicia com `--enable-experimental-web-platform-features`; o processo é controlado pelo runner, mas roda em janela externa. Nenhum código do jogo é baixado nem distribuído pelo WGR.

No modo `--probe`, a página embarcada testa APIs (inclusive a validação de um módulo memory64 e shared memory64), envia um JSON para o endpoint local `/log` e o runner o imprime no terminal. Ele encerra apenas seu próprio processo Servo. Se não houver `servoshell.exe` no caminho informado, falha sem fallback implícito.

O probe roda o Servo em modo **headless** e aguarda o JSON com `phase: "complete"` antes de relatar sucesso. O runner rejeita resultado parcial. O teste inclui pixel 2D/WebGL lido da GPU, criação de dispositivo WebGPU e escrita em buffer, instanciação de um módulo mínimo memory64 e gravação/leitura IndexedDB.

Se necessário, habilite `$env:WGR_SERVO_TRACE='1'` para visualizar os logs HTTP e os atributos `Sec-Fetch-*` sem registrar tokens ou corpos de solicitações. O `wgr-http` aceita `Sec-Fetch-Site: cross-site` **somente** em uma navegação inicial `GET` com `Sec-Fetch-Mode: navigate`, `Sec-Fetch-Dest: document`, caminho `.html`/`.htm` ou `/`, sem `Origin` e sem `Referer`; demais acessos cruzados continuam bloqueados.

## Evidência — Servo oficial Windows, 20/11/2026

- Servoshell nightly, Windows x86-64 MSVC, obtido de `https://download.servo.org/nightly/windows-msvc/servo-x86_64-windows-msvc.zip`.
- SHA-256 do ZIP verificado com checksum oficial: `cb30037a307942d8126766a00bcad184aa474fc190efc78cb13c7f1fef5f57fc`.
- Executável extraído **fora do repositório**, em `%TEMP%\wgr-servo-official-probe\servo\servo\servoshell.exe`. Sem instalação permanente.
- JSON completo do probe executado na máquina Windows:

| Verificação | Resultado |
| --- | --- |
| JavaScript, `WebAssembly`, `Worker` | disponíveis |
| `memory64` (validação **e instanciação** do módulo mínimo) | **sim** |
| `SharedArrayBuffer`, memória Wasm compartilhada, Wasm shared memory64 | **não** |
| `crossOriginIsolated` com COOP/COEP emitidos pelo servidor | **false** |
| Canvas 2D, OffscreenCanvas 2D (leitura de pixel) | **sim** |
| WebGL 1 (contexto, clear e leitura de pixel) | **sim** |
| WebGL 2 | **não** |
| WebGPU: API, adaptador, dispositivo e escrita em buffer | **sim** |
| WebAudio (API presente), Gamepad (API presente) | **sim**; som e dispositivo físico **não testados** |
| localStorage (gravar/ler), IndexedDB (gravar/ler) | **sim** |

**Resultado decisivo:** um módulo memory64 isolado funciona, mas a falta de isolamento cross-origin e de memória compartilhada impede declarar compatibilidade com um port RAGE que exige Wasm multithread. A presença de WebGPU não comprova pipeline 3D do jogo. Nem memória64 nem WebGPU foram testados com o binário GTA no Servo.

**Limites de interpretação:** as verificações cobrem APIs e operações pequenas, não renderização complexa do jogo, som audível, thread WASM funcional, gamepad físico, persistência entre novas sessões/portas, nem paridade com Chromium. O runner não comprova encapsulamento de janelas nem desempenho superior.

## Critérios para integrar ao launcher

1. Verificar em versões futuras do Servo a implementação de isolamento cross-origin/SharedArrayBuffer, WebGL2 e pipelines gráficos reais. O probe inicial acima está concluído.
2. Executar uma fixture web real (`fixtures/orbit`) e depois um port autorizado. Verificar frame, input, som, save, perda de foco, reload e perfil.
3. Projetar integração com API de embedding do Servo e evento/superfície da janela GPUI; evitar `SetParent` como atalho para transformar janela externa em WebView.
4. Implementar isolamento por jogo, allowlist de origens, credenciais, limites de processos/recursos, telemetria e encerramento limpo. O sidecar atual permite executar conteúdo externo dentro do Servo; use somente bundles confiáveis.
5. Só depois integrar no seletor de runtime e manter testes regressivos do caminho RAGE.

Não assumir consumo fixo de 100–200 MB nem 20–50 MB: ambas as cifras dependem de jogo, motor, GPU, cache e modo de build.

## Validações locais

- `cargo test -p wgr-http -p wgr-servo --locked --offline`: **9 testes HTTP + 3 testes Servo passaram**, um benchmark HTTP intencionalmente ignorado.
- O teste `servo_document_navigation_keeps_cross_site_guards` cobre os acessos aceitos e os casos bloqueados (recursos, método POST, status, `Origin` e `Referer` não autorizados).
- Os testes gerais de `wgr-core` ainda dependem do arquivo `assets/gta/zip_index.json`, excluído por alterações locais pré-existentes; não restauramos esse arquivo sem avaliar tais alterações.
