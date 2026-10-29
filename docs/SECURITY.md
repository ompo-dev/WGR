# Segurança e privacidade

O servidor de assets só faz bind em IPv4 loopback e porta alocada pelo sistema. Valida Host/Origin/Referer/Fetch-Site; não publica funções administrativas. Conteúdo só vem de roots canonicalizadas importadas ou de um índice ZIP configurado. Nomes ambíguos do Windows, ADS, traversal e ZIP Slip são rejeitados.

O cliente Rust usa TLS, sem proxy herdado ou redirects automáticos. Todos os endereços DNS são validados e fixados no cliente por conexão. Redes privadas, link-local/metadata, endereços reservados e userinfo na URL são bloqueados; HTTP loopback só após opt-in de desenvolvimento. Redirect passa pela mesma allowlist/DNS. Headers de credencial são marcados sensíveis e impedem redirects entre origens. Limites de tamanho, timeout e concorrência são explícitos.

WebView2 tem sandbox/segurança normais, sem flags Chromium de depuração ou relaxamento. Fontes remotas genéricas usam o broker descrito acima. Conteúdo local usa diretamente o listener loopback. O GTA conhecido usa a rede nativa Chromium, com as duas origens exatas `https://fiveplay.lol` e `https://cdn.fiveplay.lol` autorizadas e outros recursos recusados. Nesse caminho o TLS/DNS e o cache HTTP são do navegador, não o DNS fixado do cliente Rust. Loader/worker são adaptados somente após conferir SHA-256; mudanças de contrato falham explicitamente. A engine é obtida pelo backend Rust validado e entregue pela URL same-origin controlada, sem desativar CORS. Não há objetos COM expostos a JavaScript. Popups, downloads, notificações, câmera, microfone e clipboard por permissão são negados. O canal web/native aceita apenas relatórios tipados de diagnóstico e métricas; as mensagens da fixture só são aceitas para o jogo incluído. O harness de input nativo só é acionado no smoke opt-in e verifica o HWND próprio.

Credentials da API são guardadas no Windows Credential Manager; não são serializadas em SQLite ou manifestos e não são passadas a jogos. Logs ficam na máquina. O export de diagnóstico inclui UUIDs/capacidades locais, sem token. Remover fonte apaga sua credencial. Vários jogos têm perfis e origens separados, mesmo quando suas URLs locais usam a mesma pasta originalmente.

Uma instância por diretório de dados é garantida por lock do arquivo de runtime. Reinício recupera sessões que ficaram abertas como interrompidas. Arquivos temporários de extração/cache nunca são tratados como conteúdo íntegro.

## Limites conhecidos

O cache não implementa ETag de revalidação do ZIP remoto. O índice e sua identidade são o contrato de uma versão do arquivo; a origem deve fornecer um bundle imutável ou índice atualizado. Objetos pequenos completos validam CRC/tamanho quando disponíveis. Intervalos STORED e DEFLATE grandes não validam o CRC do membro inteiro, pois não materializam o arquivo completo. Limites de expansão e tamanho continuam aplicados. O SHA-256 de objetos em disco é verificado na abertura inicial da sessão. Acesso local malicioso aos dados do usuário está fora do isolamento do navegador.

Perfis e caches não equivalem a uma VM. O app depende das atualizações de segurança do Windows/WebView2. Não há auditoria de segurança externa concluída. `cargo audit` deve ser executado quando a ferramenta estiver disponível; a ausência é registrada, sem um resultado inventado.
