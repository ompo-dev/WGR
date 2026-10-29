# Reuso dos componentes MomorRust

Origem somente leitura: `../MomorRust`, commit `db7fcb1b6f590cdf0ba1d5323db2cac6b25f56ae`. A skill zero-write-engineering foi lida e aplicada antes das alterações. `scripts/Copy-MomorUi.ps1` produziu o plano, conferiu SHA-256, recusou destinos existentes e copiou os arquivos sem alterar a origem. Os hashes em `momor-copy-plan.json` descrevem a cópia original, antes das adaptações abaixo.

## Uso efetivo

- Button/ButtonLike: ações primárias, secundárias e navegação com foco e Tooltip originais.
- InputField: busca e formulário, incluindo o controle de revelar senha. Um adaptador ErasedEditor conecta a edição/IME do GPUI ao componente real.
- Label, ListItem e Modal: rótulos, navegação lateral e diálogos.
- DropdownMenu/ContextMenu: ordenação com teclado, Enter e Escape.
- Theme/theme_settings: One Dark, fontes do sistema e densidade padrão; Assets fornece ícones Lucide selecionados da origem.

Os crates necessários e seu código-fonte estão em `vendor/momor`. A lista de módulos compilados de `ui` exclui componentes de editor/IA/colaboração não utilizados pelo WGR, preservando os arquivos originais no snapshot.

## Adaptações locais

O MomorRust usa um fork recente do GPUI. Para GPUI 0.2.2 publicado, foram ajustadas assinaturas de foco/update, Corner em lugar de Anchor nos cantos, IDs SharedString, resolução de ícones, decoração de texto e conversão de Pixels/rems. APIs opcionais de elipse à esquerda e substituição de família de fonte em spans não existem nessa versão; o WGR não usa esses ramos. Não foram copiados flags de navegador, código de contas ou servidores do editor.

InputField ganhou `from_editor`, que usa o ponto de extensão ErasedEditor já existente e conserva a renderização do componente. Os textos de revelar/ocultar senha foram traduzidos. `crates/wgr-app/src/momor.rs` inicializa o tema, assets e atalhos originais do menu.

Os cabeçalhos GPL e o código-fonte acompanham a distribuição. Ícones mantêm o aviso ISC/MIT. Nenhuma dependência de caminho para o checkout original é usada em compilação ou execução.
