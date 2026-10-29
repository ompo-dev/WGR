# WGR Catalog API v1

`GET /.well-known/wgr.json` na origem da URL de API:

```json
{"schema_version":"1.0","publisher":"Minha biblioteca","catalog_url":"https://games.example.com/wgr/v1/games","authentication":{"type":"bearer","required":false},"allowed_asset_origins":[]}
```

`GET catalog_url?limit=50&cursor=<cursor opaco>`:

```json
{"schema_version":"1.0","items":[{"id":"meu-jogo","title":"Meu jogo","description":"Jogo WebAssembly","entry_url":"https://games.example.com/play/index.html","cover_url":null,"runtime":"webview2","version":"1.0","requirements":{"webgpu":false,"wasm_memory64":false,"shared_array_buffer":false}}],"next_cursor":null}
```

O discovery é limitado a 1 MiB, páginas a 2 MiB/200 itens, catálogo a 16 MiB/10 mil jogos/200 páginas. IDs e títulos têm até 256 bytes, descrição até 8192 bytes, URL até 4096 bytes, cursor até 2048 bytes. Cursor repetido e ID duplicado causam erro sem commit parcial. A versão suportada é `1.0`; runtime `webview2`. Campos extras são ignorados; requisitos extras preservam seu nome e bloqueiam o launch automático por não terem sido testados.

As origens do discovery, catálogo, entradas e capas precisam estar na allowlist do usuário. O servidor não consegue aumentar a política local por manifesto. Redirect é revalidado e tem limite de cinco saltos. Credenciais são vinculadas à origem da API e nunca acompanham redirect entre origens. APIs genéricas sem discovery WGR geram uma mensagem explicativa; páginas HTML podem ser adicionadas pelo modo Link.

Auth no formulário: none, Bearer, API key com nome de header ou Basic com `usuário:senha`. Autenticação exige HTTPS. O Windows Credential Manager guarda o segredo; SQLite guarda apenas uma referência opaca. Credenciais são usadas pelo catálogo e não são entregues aos jogos.

## Pacote local

`index.html` na pasta escolhida. Opcional:

```json
{"title":"Meu jogo","description":"Jogo offline","entry_url":"index.html","version":"1.0.0","requirements":{"webgpu":false}}
```

ZIP pode conter a raiz diretamente ou uma única pasta com o jogo. Limites: 50 mil membros, 4 GiB expandidos, 1 GiB por arquivo, expansão de até 500 vezes para membros acima de 1 MiB. Não são aceitos traversal, caminhos Windows especiais/ADS, links simbólicos, nomes duplicados por case ou arquivos criptografados. Um erro descarta apenas o staging próprio.

## Servidor de assets

Rotas: `/`, arquivos relativos, `/b/<build>/<path>`, `/data/<arquivo>`, `POST /data/batch`, `GET|HEAD /data/batchc/<sha1>.bin`, `GET|HEAD /__status` e `POST /log`. Não há rotas administrativas. O log do port aceita até 4 KiB e conserva somente 32 mensagens de até 1024 caracteres em RAM, visíveis no status da mesma origem. Não é gravado no disco.

Batch JSON é uma lista de `[referência, início, fim inclusivo]`, até 2000 runs/1 MiB de pedido/64 MiB de saída. Ordem é preservada, fim é truncado ao tamanho do recurso e `X-Run-Lengths` descreve a saída de cada run. `gz=1` devolve gzip; com `c=1&v=<versão>`, o cache usa SHA-1 de `versão + ':' + corpo JSON original`. `batchc` devolve os bytes gzip crus, sem Content-Encoding, conforme o loader original.

GET/HEAD têm Range único `bytes=a-b`, `a-` e `-N`. Multipart, números inválidos e intervalos impossíveis retornam 416/Content-Range. ETag permite 304. COOP/COEP/CORP são aplicados inclusive em erros. Para arquivos, ETag e Cache-Control são políticas próprias do WGR; isso difere do Node e é registrado na comparação.

[JSON Schema](protocol/wgr-catalog-v1.schema.json).
