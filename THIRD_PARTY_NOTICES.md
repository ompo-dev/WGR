# Proveniência e licenças

WGR: GPL-3.0-or-later. Texto integral em `LICENSE`. Fixture Órbita (HTML/CSS/JS/Wasm/dados): código original, licença MIT em `fixtures/orbit/LICENSE`.

## GPUI

Usamos `gpui 0.2.2` publicado no crates.io, Apache-2.0. O backend de edição de `crates/wgr-app/src/input.rs` foi adaptado do exemplo `examples/input.rs`, de Zed Industries e contribuidores. Cópia do Apache-2.0 em `assets/licenses/APACHE-2.0.txt`.

## MomorRust

Componentes e tema copiados de [ompo-dev/MomorRust](https://github.com/ompo-dev/MomorRust), commit `db7fcb1b6f590cdf0ba1d5323db2cac6b25f56ae`, GPL-3.0-or-later. Os cabeçalhos originais foram preservados em `vendor/momor/`. O plano de cópia registra os hashes da origem em `docs/momor-copy-plan.json`; alterações de compatibilidade com GPUI publicado e a integração InputField estão descritas em `docs/MOMOR-UI.md`. Nenhuma alteração foi feita no repositório original. Os ícones Lucide selecionados preservam o aviso ISC/MIT em `assets/momor/icons/LICENSES`.

## 5-web-port

[Tyduss/5-web-port](https://github.com/Tyduss/5-web-port), commit `05b002fb30fad868db0afe9457061c792b830608`, declara `license: MIT` no seu package.json. Snapshots sem alteração: `assets/gta/play.html`, `assets/gta/zip_index.json` e `tests/reference/5-web-port-server.cjs` (servidor somente para teste). A cópia local não contém um LICENSE separado com copyright do titular; registramos a declaração MIT e a proveniência sem inventar autoria. A declaração do host não concede direitos sobre o jogo ou seus assets. Nenhum bundle proprietário, logotipo, fonte, música, shader ou engine de GTA é incluído.

Os contratos de Range/batch/ZIP foram reimplementados em Rust, com adaptações de segurança e limites explicitados na documentação. A biblioteca SHA-1 é usada apenas para a chave legada de batch; integridade de objetos usa SHA-256.

## Dependências

## Muguet: adaptador nativo RAGE

Fonte: [c22dev/muguet](https://github.com/c22dev/muguet), commit `728c195ab02c746a1f2d626238bfe030b2f561fb`, GPL-3.0-or-later. Copyright e licença originais preservados em `vendor/muguet/`; o fork local registra a proveniência em `WGR-PROVENANCE.md`. Adaptações WGR: Windows/DirectX/Vulkan, Wasmtime 50, HWND filho, controle de processo, transporte Rust sob demanda e cache limitado. Os patches upstream de winit/wgpu preservam suas licenças. O áudio Opus usa a biblioteca nativa distribuída por `audiopus_sys`, com sua licença BSD. Nenhum dado, shader ou binário do jogo acompanha esta cópia.

## Dependências resolvidas

`Cargo.lock` fixa as versões. `docs/DEPENDENCIES.md` e `assets/licenses/dependencies/` são gerados por `scripts/Collect-Licenses.ps1` com nomes, versões, expressões SPDX e textos de licença disponíveis nos pacotes Windows resolvidos. O código-fonte WGR e instruções de build acompanham o pacote portable. Microsoft WebView2 Evergreen é instalado e licenciado separadamente pela Microsoft.
