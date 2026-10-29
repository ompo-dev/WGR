# Arquitetura

## Fronteiras

- `wgr-core`: entidades, estado de sessão, SQLite WAL/migração, catálogo, políticas de URL/DNS, cofre Windows, ZIP, thumbnails no app e cache com endereçamento SHA-256.
- `wgr-http`: Axum/Tokio, listener `127.0.0.1:0`, validação Host/Origin/Referer/Fetch-Site, respostas com isolamento, streaming em chunks, GET/HEAD/Range, batch e status sem segredos.
- `wgr-app`: GPUI nativo, composição, forms/CRUD, tarefas de I/O, WebView2 COM, HWND filho, foco/DPI/fullscreen, diagnóstico e smoke opt-in.

Os componentes UI/theme do MomorRust foram copiados com hashes conferidos e adaptados ao GPUI publicado no crates.io. A biblioteca real renderiza botões, campos, listas, menus, modal e tooltips; o backend de edição implementa a interface ErasedEditor do InputField. O workspace completo do editor não é necessário. O host COM foi implementado no WGR, sem loop Wry/Tao concorrente. Veja MOMOR-UI.md.

## Fluxo de uma sessão

`Idle → Resolving → Checking → Preparing → Loading → Running → Stopping → Stopped` é validado no domínio. Falha/crash tem estado explícito. `Running` exige sinal de frame da fixture suportada; páginas genéricas permanecem com execução não confirmada. O canal `game-fps` do GTA é informação do próprio port, sem prova independente de jogabilidade.

O launcher resolve o jogo e cria um servidor quando local/GTA. O player abre uma página de probe controlada, verifica requisitos e navega para a entrada. Um segundo probe coleta o contexto do documento efetivo. Objetos COM permanecem na thread da janela. HTTP, download, descompressão e operações mais demoradas usam o runtime; eventos retornam à UI. O timer de integração existe durante operações/sessões/capas em carga e para ao ficar ocioso.

Perfis ficam em `profiles/<game UUID>`. Pacotes locais usam origem real `http://127.0.0.1:<porta persistida>`, incluindo workers aninhados. O GTA padrão usa `https://fiveplay.lol`, com allowlist exata para o site e seu CDN; engine/loader têm adaptação restrita. A resposta preserva MIME, Range e políticas COOP/COEP/CORP. Não há interface web para CRUD, arquivos do usuário ou comandos nativos. Popups, downloads e permissões sensíveis são negados.

## Assets/cache

Arquivos locais são abertos sob uma root autorizada/canonicalizada. Respostas são transmitidas em chunks de 1 MiB. STORED permite leitura direta de intervalos. Dados do jogo e objetos grandes DEFLATE mantêm cursores de descompressão em RAM: pulam bytes anteriores sem gravá-los e guardam somente janelas solicitadas em um LRU de 128 MiB compartilhado com batches. Há no máximo 16 cursores retidos, oito leituras ativas e quatro requisições de origem simultâneas. Pedidos para trechos anteriores fora do LRU reiniciam a descompressão; essa é uma limitação do acesso a ZIP DEFLATE sem índice de checkpoints.

No fallback ZIP, somente objetos pequenos de código/arte, até 1 MiB, podem ser materializados em disco. Recebem CRC/tamanho quando disponíveis, promoção atômica e SHA-256 do cache. Dados `data/` do ZIP nunca usam essa materialização. O índice inteiro participa da identidade do cache. No site original, a engine completa usa o cache Rust e os intervalos de dados têm cache persistente separado de até 1 GiB, por versão e arquivo; o worker RAM continua limitado a 48 MiB. `staging` descarta apenas arquivos `.partial` com nome UUID ao abrir exclusivamente o diretório de runtime. Saves não vivem nesse diretório.

SQLite do cache guarda referências e LRU. Objetos usados ficam pinados até o encerramento da sessão; quota sem espaço retorna erro. Partial não é servido como objeto completo. O cache do WebView2, shaders, perfis e saves são geridos pelo navegador e não se confundem com este cache.

O servidor local usa uma porta persistida por jogo. O WebView2 acessa essa origem real diretamente, incluindo importScripts e workers aninhados. A antiga origem virtual interceptada falhava em workers aninhados e exigia spool; foi retirada do fluxo local. Requisições pendentes são canceladas ao fechar a sessão. O launcher usa o swap chain HWND do GPUI, preservando a apresentação do WebView filho acima do conteúdo da janela.

## Referências locais auditadas

O player da engine ainda é WebView2. O contrato para um host Rust e a avaliação de Servo/Oxide/Blitz estão em [NATIVE-RUNTIME.md](NATIVE-RUNTIME.md).

MomorRust: `db7fcb1b6f590cdf0ba1d5323db2cac6b25f56ae`.
5-web-port: `05b002fb30fad868db0afe9457061c792b830608`.

As referências são somente leitura. A distribuição contém snapshots do índice e de `play.html`; o servidor Node original está somente em testes para comparação. Nenhum caminho absoluto dessas cópias está compilado como dependência de runtime.
