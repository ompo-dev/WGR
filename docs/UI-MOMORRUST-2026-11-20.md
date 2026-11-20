# Interface do WGR — componentes MomorRust

## Origem e escopo

A interface do WGR foi reorganizada usando exclusivamente componentes e padrões já presentes no repositório local `MomorRust`:

`C:\Users\maico\OneDrive\Documents\GitHub\MomorRust`

O código original do MomorRust permaneceu **somente para leitura**. A implementação do WGR utiliza os crates existentes e já integrados em `vendor/momor`, especialmente `ui`, `theme`, `ui_input` e `menu`, com GPUI. Não foi criado outro sistema de componentes nem adicionado um framework de UI.

## Componentes e padrões reutilizados

| Elemento do WGR | Componente / API do MomorRust |
| --- | --- |
| Estrutura e navegação lateral | `ListItem`, `Label`, `Icon`, `Toggleable`, tema ativo |
| Biblioteca | `Headline`, `Button`, `Chip`, `Indicator`, `IconButton`, menus de ordenação |
| Cartões e detalhes de jogos | `Button`, `Icon`, `Chip`, `Headline`, `Label`, `Indicator` |
| Fontes e catálogos | `Chip`, `Icon`, `Headline`, `Indicator`, `Button` |
| Configurações e diagnóstico | `Headline`, `Label`, `Button`, `Icon` e tokens do tema |
| Importação, edição e confirmação | `Modal`, `Button`, campos existentes de `ui_input` |
| Cores e estados | `theme::ActiveTheme`, `cx.theme().colors()` e `cx.theme().status()` |

Os componentes vêm de `MomorRust/crates/ui/src/components/`; a tipografia e as cores vêm de `crates/ui/src/styles/` e `crates/theme/src/styles/`. O WGR já inicializava esse tema no arquivo `crates/wgr-app/src/momor.rs`; a UI anterior ainda usava uma paleta fixa própria, agora substituída pelos tokens de tema.

Somente SVGs de ícones efetivamente referenciados pela interface foram copiados, sem alteração, de `MomorRust/assets/icons` para `WGR/assets/momor/icons`. O arquivo existente `assets/momor/icons/LICENSES` permanece junto dos ícones. Nenhum arquivo de fonte tipográfica foi copiado.

## Fluxos preservados

- Pesquisa e filtros da biblioteca, favoritos, ordem e seleção de jogos.
- Execução e retorno do player, inclusive o GTA V pelo runtime nativo.
- Cadastro, edição, sincronização, reordenação e remoção de fontes.
- Assistente de importação, autorização de origem, credenciais e confirmação.
- Configurações de tela cheia e cache, abertura de diretório, diagnóstico e exportação.
- Atalhos de teclado, estados de carregamento/erro e foco dos campos.

Esta alteração é de apresentação. Não modifica shaders, streaming, qualidade gráfica, dados de usuário ou o contrato da engine. A biblioteca usa um a três cartões por linha conforme a largura disponível, respeitando a largura mínima de janela já definida no WGR.

## Verificação

Executar, na raiz do WGR:

```powershell
cargo fmt -p wgr-app -- --check
cargo clippy -p wgr-app --offline -- -D clippy::correctness
cargo test -p wgr-app --offline
cargo build -p wgr-app --release --offline
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/Smoke.ps1 -Executable target/release/WGR.exe
```

Capturas da biblioteca, fontes, configurações, diagnóstico, detalhes e assistente de importação foram produzidas por `scripts/Capture-Wgr-Target.ps1` para o HWND específico do processo do WGR. Elas ficam em `artifacts/momor-ui-*.png` (ignoradas pelo Git). O teste usa `artifacts/smoke-data`, separado dos dados do usuário.

O resultado de smoke da fixture não prova, por si só, a jogabilidade de um título comercial ou FPS sustentado em cenas intensas.

## Resultado da validação

- `cargo fmt -p wgr-app -- --check`: aprovado.
- `cargo clippy -p wgr-app --offline -- -D clippy::correctness`: aprovado.
- `cargo test -p wgr-app --offline`: **9 testes aprovados**.
- `cargo build -p wgr-app --release --offline`: aprovado.
- `scripts/Smoke.ps1 -Executable target/release/WGR.exe`: **3/3 execuções aprovadas**, primeiro frame mediano de **103 ms** (mínimo 90 ms, máximo 106 ms), save restaurado.
- Capturas por HWND do executável release mostram biblioteca, fontes, importação, detalhes, configurações e diagnóstico. Ajustes finais de alinhamento passaram na compilação e smoke posteriores.

O executável atualizado está em `target/release/WGR.exe`. A versão antiga de `WGR.exe` na raiz foi preservada porque havia uma instância desse executável aberta durante a validação; não houve interrupção da sessão do usuário.
