# Testes e reprodução

## Automatizados

Execute `scripts/Validate.ps1`. Ele roda fmt, clippy, testes, release e smoke. Node é requisito apenas do teste de contrato; a versão upstream, sem alterações, está em `tests/reference/5-web-port-server.cjs` e recebe exclusivamente fixtures originais.

Os testes cobrem persistência/reabertura e mudança da pasta portable, importação transacional, sync idempotente de duas páginas, ordenação completa, transições de sessão, URL/rede privada/redirect, ZIP Slip, cache/quota/pins/SHA-256, thumbnails, preview sem commit, GTA sem origens autorizadas, build descoberto no índice real, Range/HEAD/304/416/arquivos vazios, Host/Origin/Referer/traversal/tamanho, gzip/batch (JSON e texto do worker original)/cache SHA-1 e ZIP STORED/DEFLATE com upstream local.

O teste Node compara nove operações, códigos, bytes e cabeçalhos do contrato. O gzip é byte-exato para a fixture pequena testada, incluindo o byte de plataforma do header; não inferimos igualdade de qualquer stream produzido por compressores diferentes. O WGR deliberadamente conserva os cabeçalhos de isolamento em erros e usa sua própria política de cache.

## Smoke Windows real

```powershell
pwsh -File scripts/Smoke.ps1 -Executable WGR.exe
```

Uma pasta de dados de teste fica em `artifacts/smoke-data`, separada dos dados normais. O programa abre a janela GPUI, captura sua própria janela, abre Órbita em WebView2, verifica primeiro frame/Worker/Range/batch/IndexedDB, envia uma seta via mensagens Win32 **somente para o HWND descendente do player**, clica no botão de áudio nesse mesmo HWND e submete comandos WebGPU. A captura do player usa a API CapturePreview. Não se usa endpoint de debug nem flags de segurança desativadas.

O script reabre o mesmo perfil e verifica que o jogo restaura o save. Entrada automatizada por Win32 não substitui um teste humano de teclado, mouse, pointer lock ou gamepad físico. AudioContext `running` comprova processamento do worklet, não mede som no alto-falante. WebGPU indisponível é explicitamente registrado, sem declarar renderização GPU bem-sucedida.

## Checklist manual

Para a origem GTA, `scripts/Benchmark-Gta.ps1 -Fps 60 -Name baseline` e depois `-Fps 0 -Name uncapped` aguardam `world:true` e medem 30 segundos, com recursos da árvore do próprio WGR. Posição Sandbox aleatória e cache aquecido em momentos diferentes impedem tratar essa sequência como comparação determinística. `scripts/Audit-Wasm.mjs` inspeciona memória/importações sem instanciar a engine. Os testes `tests/session-observer.mjs` e `tests/ram-io.mjs` verificam preparação antes de iniciar workers, transferência do módulo/canvas, deduplicação de relatórios, prioridade de demanda e planejamento do boot.

- Abrir vazio/perfil novo; CRUD de jogo e fonte; cancelamento e erros de importação.
- Tab/Shift+Tab, clipboard, acentos/IME, Ctrl+K/Ctrl+N, foco do player e Ctrl+Shift+B.
- Resize e DPI 100%, 125%, 150%, 200%; fullscreen em mais de um monitor.
- Conectar gamepad, hot-plug de áudio, mute, pointer lock e liberação de mouse.
- Desconectar rede, API fora do ar, resposta corrompida, GPU/browser crash, disco cheio e cancelamento no download.
- GTA com arquivos legítimos: engine real, primeiro quadro, input, áudio e limites de GPU.
- Comparar Edge/Chrome com resolução, GPU, port, cache e aquecimento idênticos em três execuções. Nenhuma superioridade é presumida.

Não marcamos estes itens manuais como aprovados apenas por compilar. Ver `VALIDATION.md` para evidência concreta desta execução.
