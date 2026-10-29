# Armazenamento e memória

O navegador também usa cache em disco. O WGR conserva dados reutilizáveis com quota, sem apagar tudo a cada abertura.

| Camada no GTA original | Limite |
| --- | --- |
| Intervalos persistentes | 1 GiB de payload em IndexedDB `wgr-block-cache-v1`, LRU atômico e chaves por versão/arquivo/offset |
| Worker RAM | 48 MiB, blocos de 4 KiB, demanda prioritária; até 32 tarefas ativas, oito especulativas |
| HTTP Chromium | `--disk-cache-size=268435456` (256 MiB); não limita o perfil inteiro |
| Cache Rust | 64 MiB por padrão, configurável até 256 MiB; inclui a engine de 60,27 MiB |
| Saves | Banco próprio da engine, preservado pela limpeza |

O banco acrescenta índices e metadados além do payload. Perfis, JIT, shaders e saves também usam disco. O OPFS original sem quota própria fica desativado nessa adaptação. A origem e o perfil por UUID persistem entre processos.

O boot contém aproximadamente 485 MiB no perfil de pouca memória ou 555 MiB no normal, antes do alinhamento. Seus intervalos são antecipados no cache persistente; o LRU RAM permanece limitado. Dados fora do cache ainda precisam da origem. A quota não mantém todos os assets de uma cidade inteira.

`--clear-cache` limpa o cache Rust. `--clear-profile-cache` limpa HTTP/CacheStorage, OPFS `gamedata` antigo e somente `wgr-block-cache-v1`; preserva `gta5-userdata` e demais saves. A limpeza é explícita.

O fallback ZIP mantém streaming em RAM: LRU de intervalos/batches de 128 MiB, 16 cursores, 128 checkpoints; não materializa RPFs grandes no disco. Os testes verificam dados exatos e ausência de temporários de conteúdo.

A compilação usa um perfil release temporário fora do OneDrive e um compilador. `scripts/Validate.ps1` copia o executável e limpa intermediários. Caches globais do driver, arquivos originais e a configuração de paginação não são removidos.

A engine exige **3 GiB mínimos de memória linear compartilhada**, com máximo declarado de 16 GiB. O máximo não é consumo físico; memória linear, compromisso e páginas residentes são medidas distintas. Não somar a região compartilhada como se cada thread tivesse uma cópia. Renderer, GPU e shaders acrescentam consumo. Veja [NATIVE-RUNTIME.md](NATIVE-RUNTIME.md).

Sob pressão de compromisso, o Windows pode ampliar `pagefile.sys`. Sua configuração não foi alterada. As medições usam uma engine por vez e acompanham apenas a árvore do WGR. [Documentação Microsoft sobre paginação](https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/introduction-to-the-page-file).
