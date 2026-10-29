# Investigação de carregamento

O HAR do usuário mostrou que o navegador usa `https://fiveplay.lol`, com engine em `https://cdn.fiveplay.lol`. O caminho anterior pelo ZIP Archive.org não reproduzia essa origem. Cookies e conteúdo bruto do HAR não são distribuídos.

## Causas verificadas e correções

- **Origem/cache diferentes:** 606 pedidos no HAR, aproximadamente 73 MB atendidos pelo cache de disco e muitas leituras 206. O WGR usava ZIP e `nocache=1`, repetindo leituras e prefixos DEFLATE. A entrada padrão migrou para a origem original, preservando UUID, saves, favorito e histórico; a configuração anterior fica salva.
- **CORS na engine:** `game.wasm` redireciona para o CDN sem `Access-Control-Allow-Origin`. O teste registrou `Failed to fetch`. O backend Rust validado obtém a engine, confere o cabeçalho WASM e a entrega numa URL same-origin controlada. A compilação streaming precede GPU/IO; o módulo compilado é compartilhado. A segurança não é desativada.
- **Workers aninhados:** a interceptação WebView2 não cobre suficientemente todos os workers internos. O loader de hash conhecido recebe um IO worker Blob adaptado. Mudanças de hash/contrato falham explicitamente. Scripts proprietários são buscados em runtime e não acompanham a distribuição.
- **Cache desativado:** intervalos persistentes de até 1 GiB substituem o armazenamento sem quota própria, separados dos saves, com 48 MiB RAM e HTTP configurado para 256 MiB. Três processos da fixture verificam reabertura, versão e quota.
- **Prefetch insuficiente:** a versão RAM-only antecipava somente os primeiros 4 KiB de cada arquivo. Com armazenamento persistente, os intervalos completos do boot são antecipados por até oito tarefas, alinhados, unidos e divididos em blocos de até 128 KiB. Demanda mantém prioridade. A quantidade de tarefas não cresce com o manifesto. RAM-only continua antecipando cabeçalhos.
- **Teto de FPS:** a página impõe `-frameLimit=1`, cerca de 60 FPS. O padrão agora usa `fps=0`, preservando preferências explícitas. Retirar o teto não demonstra, sozinho, aceleração.

## O que significa 76%

`First frame` e FPS positivo ainda podem pertencer ao loading. O GPU worker emite `world:true` depois de detectar cena real e esperar pipelines: ao menos 1,5 s sem novos skips, com prazo de até 60 s. A página ainda faz uma transição. O harness agora aguarda mundo, mais quatro segundos para capturar, e mede 30 segundos após o evento.

`game-fps` conta submissões do port; não é contador independente de apresentação física nem prova de input/áudio. A porcentagem é estimativa de etapas.

## Reprodução

`scripts/Benchmark-Gta.ps1 -Fps 60 -Name baseline` e depois `-Fps 0 -Name uncapped` usam o mesmo perfil e somente uma instância WGR. A outra aba do jogo deve estar fechada. Os relatórios contêm cache, memória, configuração efetiva e FPS após mundo. Sandbox escolhe uma posição aleatória: comparar locais diferentes é uma medição exploratória, não benchmark determinístico da cidade. Não foi concluída comparação contra Edge/Chrome.

O fallback ZIP mantém blocos de 4 KiB, conexões reutilizadas, checkpoints e limites de RAM; seus testes são separados. Consulte [VALIDATION.md](VALIDATION.md) e o [contrato nativo](NATIVE-RUNTIME.md).
