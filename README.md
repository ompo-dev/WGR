# WGR — Web Game Runtime

Aplicativo Windows em Rust para uma biblioteca privada de ports de jogos. Launcher GPUI/Momor, SQLite e execução WASM por Wasmtime. O player padrão não carrega WebView2, páginas ou iframe. Sem conta ou telemetria remota.

## Abrir

Use `WGR.exe` na raiz, mantendo `wgr-rage.exe` ao lado para o adaptador GTA. `scripts/Package.ps1 -SkipBuild` gera a distribuição portable com documentação, licenças e código-fonte. O caminho padrão não requer WebView2. O adaptador GTA está em validação; consulte as evidências antes de considerá-lo jogável.

A fixture original **Órbita** funciona sem internet: Wasmtime executa `math.wasm` e GPUI desenha a cena nativa. Setas/WASD e clique movimentam o objeto; o save é restaurado entre sessões. Esta fixture comprova o host Rust, sem representar um benchmark do GTA ou paridade completa com a versão web da fixture.

## Biblioteca e fontes

- **Adicionar jogo → Link do jogo:** cadastra metadados e verifica o acesso. Uma página HTML não vira um port nativo automaticamente; o player exige binário e adaptador compatível.
- **API de biblioteca:** discovery WGR Catalog API v1 em `/.well-known/wgr.json`. Bearer, header customizado de API key ou Basic em HTTPS. O formulário oferece uma prévia antes de gravar. Sincronizações são atômicas e idempotentes e preservam favoritos, IDs e histórico. Endpoints e capas de outras origens exigem allowlist preenchida pelo usuário.
- **Pacote local:** pasta com `index.html`, ou ZIP com essa estrutura. `wgr.game.json` é opcional. A pasta original nunca é alterada; ZIPs são validados e extraídos para uma pasta própria. Não executamos programas nativos do pacote.
- **GTA:** o manifesto `ports/fiveplay-gta5.port.json` identifica o binário pelo SHA-256 e as origens dos dados/shaders. O host nativo executa o binário diretamente e lê dados por intervalos. O código Rust do adaptador reutiliza Muguet, com port para Windows/DirectX/Vulkan. Não instala o ZIP completo nem abre o site. Outros ABIs precisam de seus próprios adaptadores.

Detalhes permite editar nome, descrição, entrada e capa local, verificar arquivos, favoritar, reordenar e remover registros. A tela Fontes permite editar, favoritar, reordenar, sincronizar catálogos e remover fontes. Remover um registro preserva arquivos originais e saves. A biblioteca é virtualizada; capas são reduzidas a 320/640 pixels e carregadas conforme os cards aparecem.

## Player e armazenamento

`Ctrl+Shift+B` retorna à biblioteca, inclusive se o jogo capturar Esc. `F11` alterna tela cheia. No launcher, `Ctrl+K` busca, `Ctrl+N` adiciona e Tab/Shift+Tab percorrem controles. Volume/mute do player não altera opções internas do jogo.

Por padrão, dados ficam em `%LOCALAPPDATA%\WGR`. O host GTA nativo mantém código compilado, metadados, dados em cache e saves em `native-rage/`; os perfis do backend anterior permanecem separados. A feature explícita `web-compat` conserva o player antigo para diagnóstico. Os catálogos e credenciais continuam gerenciados pelo launcher Rust.

O transporte GTA nativo usa cache persistente de até **2 GiB**, descontando binário e código compilado; cache quente de 48 MiB e quatro leituras em paralelo. Buffers temporários de leitura e memória da engine são adicionais. O backend anterior conserva seu cache de intervalos de 2 GiB para diagnóstico, sem ser iniciado pelo player padrão. Memória virtual do Windows e caches do driver são medições separadas.

`WGR.exe --clear-cache` limpa o cache Rust pelo próprio gerenciador. `WGR.exe --clear-profile-cache` limpa cache HTTP/CacheStorage, a pasta OPFS de dados e somente o banco de intervalos `wgr-block-cache-v1`, sem iniciar a engine nem remover os saves. A limpeza é explícita: não acontece a cada abertura. Logs locais giram diariamente, com sete arquivos mantidos. Capas e pacotes locais importados são conteúdo escolhido pelo usuário e ficam separados do cache. Não há upload automático de diagnóstico.

## Compilar e validar no Windows

Requisitos de desenvolvimento: Rust 1.98.1, Visual Studio Build Tools com C++/SDK Windows e CMake para Opus. Node 20+ é usado nos testes do backend anterior. O produto nativo não exige Node ou WebView2.

```powershell
pwsh -File scripts/Validate.ps1
pwsh -File scripts/Package.ps1 -SkipBuild
```

O validador compila em uma pasta temporária fora do OneDrive, copia os executáveis `WGR.exe` e `wgr-rage.exe` e limpa os intermediários. `-KeepBuild` conserva a pasta. O workspace tem quatro crates; o adaptador RAGE é um fork GPL separado em `vendor/muguet`. Não há dependência em tempo de execução dos diretórios de referência MomorRust ou 5-web-port.

## Estado e limites

As evidências da execução local ficam em `artifacts/` e são resumidas em [VALIDATION.md](docs/VALIDATION.md). O aplicativo é uma primeira versão funcional. O loader real do GTA foi obtido pelo servidor Rust; isso, sozinho, não comprova jogabilidade. As APIs e os limites reais do WebView2 são testados no contexto da sessão, incluindo uma segunda medição na página do jogo. Uma página carregada não é registrada como primeiro frame.

O broker remoto suporta GET/HEAD/POST, origens autorizadas e corpos limitados (1 MiB de pedido, 128 MiB por resposta). Recursos locais e o GTA conhecido usam o caminho de rede nativo do navegador; loader e engine do GTA têm adaptação verificada. Veja [segurança](docs/SECURITY.md). O broker genérico não implementa WebSockets nem encaminha cookies de autenticação de páginas ao cliente Rust.

O player padrão usa Wasmtime/GPUI e o adaptador nativo RAGE usa Wasmtime/wgpu/CPAL. Isso não torna automaticamente qualquer binário compatível nem transforma o código do jogo em Rust. Consulte [execução nativa](docs/NATIVE-RUNTIME.md). Desempenho, jogabilidade GTA, gamepad físico, hot-plug de áudio, perda de GPU e matriz de DPI precisam de evidências específicas.

## Documentação

[Arquitetura](docs/ARCHITECTURE.md) · [Protocolo](docs/API.md) · [Compatibilidade](docs/COMPATIBILITY.md) · [Testes](docs/TESTING.md) · [Segurança](docs/SECURITY.md) · [Licenças](THIRD_PARTY_NOTICES.md).

Código WGR e componentes MomorRust: GPL-3.0-or-later. A interface usa os componentes reais Button, InputField, Label, ListItem, Modal, DropdownMenu/ContextMenu e Tooltip do MomorRust, com seu tema e ícones. A cópia validada e as adaptações estão registradas em [MOMOR-UI.md](docs/MOMOR-UI.md). Fixture Órbita: MIT. GPUI e o backend de edição de texto adaptado: Apache-2.0. Os trechos do host 5-web-port seguem a declaração MIT do `package.json` upstream, com proveniência registrada. Nenhuma engine, arte, áudio ou arquivo proprietário de GTA é distribuído.
