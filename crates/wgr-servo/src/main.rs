//! Servo compatibility runner. Sidecar experiment; not an embedded WebView.
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
use wgr_core::assets::{DiskCache, inspect_bundle};
use wgr_http::{AssetServer, ServerConfig};

#[derive(Debug, PartialEq, Eq)]
enum Task {
    Probe,
    Bundle(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    servo: PathBuf,
    task: Task,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut args = args.into_iter();
    let mut servo = None;
    let mut task = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--servo" => {
                ensure!(servo.is_none(), "--servo repetido");
                servo = Some(PathBuf::from(
                    args.next().context("Informe o caminho de servoshell.exe")?,
                ));
            }
            "--probe" => {
                ensure!(task.is_none(), "Escolha apenas --probe ou --bundle");
                task = Some(Task::Probe);
            }
            "--bundle" => {
                ensure!(task.is_none(), "Escolha apenas --probe ou --bundle");
                task = Some(Task::Bundle(PathBuf::from(
                    args.next().context("Informe a pasta do jogo")?,
                )));
            }
            _ => bail!("Argumento desconhecido: {arg}"),
        }
    }
    Ok(Args {
        servo: servo.context("Informe --servo <caminho para servoshell.exe>")?,
        task: task.context("Informe --probe ou --bundle <pasta>")?,
    })
}

fn entry_url(origin: &str, entry: &str) -> Result<String> {
    let mut url = url::Url::parse(&format!("{origin}/"))?;
    wgr_core::security::safe_relative(entry)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Origem sem segmentos"))?
        .extend(entry.split('/'));
    Ok(url.to_string())
}

fn spawn_servo(executable: &Path, url: &str, probe_image: Option<&Path>) -> Result<Child> {
    ensure!(
        executable.is_file(),
        "servoshell não encontrado: {}. Compile ou instale Servo separadamente.",
        executable.display()
    );
    let mut command = Command::new(executable);
    command
        .current_dir(executable.parent().context("Pasta de servoshell ausente")?)
        .arg("--enable-experimental-web-platform-features");
    if let Some(image) = probe_image {
        command
            .arg("--headless")
            .arg(format!("--output={}", image.display()));
    }
    command
        .arg(url)
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("Falha ao iniciar {}", executable.display()))
}

async fn await_probe(server: &AssetServer, child: &mut Child) -> Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?;
    let status_url = format!("{}/__status", server.origin);
    let mut last_status = serde_json::Value::Null;
    let mut initial = None;
    for _ in 0..100 {
        if let Ok(response) = client.get(&status_url).send().await
            && response.status().is_success()
            && let Ok(status) = response.json::<serde_json::Value>().await
            && let Some(logs) = status["io_logs"].as_array()
        {
            for log in logs.iter().rev().filter_map(|value| value.as_str()) {
                if let Some(json) = log.strip_prefix("WGR_SERVO_PROBE:")
                    && let Ok(value) = serde_json::from_str::<serde_json::Value>(json)
                    && value["kind"] == "wgr-servo-probe-v1"
                {
                    if value["phase"] == "complete" {
                        return Ok(value);
                    }
                    if value["phase"] == "initial" {
                        initial = Some(value);
                    }
                }
            }
            last_status = status;
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "Servo encerrou antes de completar diagnóstico: {status}. Fase inicial: {}. HTTP: {last_status}",
                initial.unwrap_or(serde_json::Value::Null)
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    bail!(
        "Servo não completou diagnóstico em 20 segundos. Fase inicial: {}. HTTP: {last_status}",
        initial.unwrap_or(serde_json::Value::Null)
    )
}

async fn run(args: Args) -> Result<()> {
    let temporary = tempfile::tempdir().context("Pasta temporária do experimento Servo")?;
    let probe = matches!(&args.task, Task::Probe);
    let (root, entry) = match args.task {
        Task::Probe => {
            let root = temporary.path().join("probe");
            std::fs::create_dir_all(&root)?;
            std::fs::write(
                root.join("index.html"),
                include_bytes!("../../../fixtures/servo-probe/index.html"),
            )?;
            (root, "index.html".to_string())
        }
        Task::Bundle(folder) => {
            let info = inspect_bundle(&folder).context("Bundle local inválido")?;
            (info.root, info.entry)
        }
    };
    let cache = Arc::new(DiskCache::open(
        temporary.path().join("cache"),
        4 * 1024 * 1024,
    )?);
    let server = AssetServer::start(ServerConfig {
        root,
        archive: None,
        cache,
    })
    .await?;
    let result = async {
        let url = entry_url(&server.origin, &entry)?;
        if probe {
            eprintln!("WGR Servo probe URL: {url}");
        }
        let probe_image = temporary.path().join("servo-probe.png");
        let mut child = spawn_servo(&args.servo, &url, probe.then_some(probe_image.as_path()))?;
        if probe {
            let result = await_probe(&server, &mut child).await;
            // Only stop the process created for this diagnostic run.
            let _ = child.kill();
            let _ = child.wait();
            println!("{}", serde_json::to_string_pretty(&result?)?);
        } else {
            println!("Servo abriu {url}. Feche a janela para encerrar a sessão.");
            let status = child.wait()?;
            ensure!(status.success(), "Servo encerrou com {status}");
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    server.shutdown().await;
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var_os("WGR_SERVO_TRACE").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("wgr_http=debug,wgr_core=info")
            .try_init();
    }
    let raw = std::env::args().skip(1).collect::<Vec<_>>();
    if raw.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Uso: wgr-servo --servo <servoshell.exe> --probe\n\
             ou:  wgr-servo --servo <servoshell.exe> --bundle <pasta com index.html>\n\
             Abre uma janela Servo externa; não substitui o runtime nativo do WGR."
        );
        return Ok(());
    }
    run(parse_args(raw)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_an_explicit_engine_and_a_single_mode() {
        assert!(parse_args(["--probe".to_string()]).is_err());
        assert!(parse_args(["--servo".to_string(), "servo.exe".to_string()]).is_err());
        assert!(
            parse_args([
                "--servo".to_string(),
                "servo.exe".to_string(),
                "--probe".to_string(),
                "--bundle".to_string(),
                "game".to_string(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_args([
                "--servo".to_string(),
                "servo.exe".to_string(),
                "--bundle".to_string(),
                "game".to_string(),
            ])
            .unwrap()
            .task,
            Task::Bundle(PathBuf::from("game"))
        );
    }

    #[test]
    fn escapes_local_entries_and_rejects_traversal() {
        let url = entry_url("http://127.0.0.1:1234", "folder #1/index.html").unwrap();
        assert_eq!(url, "http://127.0.0.1:1234/folder%20%231/index.html");
        assert!(entry_url("http://127.0.0.1:1234", "../private.html").is_err());
        assert!(entry_url("http://127.0.0.1:1234", "http://example.com").is_err());
    }

    #[tokio::test]
    async fn reports_capabilities_through_the_isolated_local_server() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("probe");
        std::fs::create_dir_all(&root)?;
        std::fs::write(
            root.join("index.html"),
            include_bytes!("../../../fixtures/servo-probe/index.html"),
        )?;
        let cache = Arc::new(DiskCache::open(tmp.path().join("cache"), 1024 * 1024)?);
        let server = AssetServer::start(ServerConfig {
            root,
            archive: None,
            cache,
        })
        .await?;
        let client = reqwest::Client::builder().no_proxy().build()?;
        let response = client
            .get(entry_url(&server.origin, "index.html")?)
            .send()
            .await?;
        ensure!(response.status().is_success(), "Fixture indisponível");
        assert_eq!(
            response.headers()["cross-origin-opener-policy"],
            "same-origin"
        );
        assert_eq!(
            response.headers()["cross-origin-embedder-policy"],
            "require-corp"
        );
        assert!(response.text().await?.contains("WGR_SERVO_PROBE:"));
        let log = r#"WGR_SERVO_PROBE:{"kind":"wgr-servo-probe-v1","worker":true}"#;
        let post = client
            .post(format!("{}/log", server.origin))
            .body(log)
            .send()
            .await?;
        ensure!(post.status().is_success(), "Registro do probe rejeitado");
        let status: serde_json::Value = client
            .get(format!("{}/__status", server.origin))
            .send()
            .await?
            .json()
            .await?;
        assert!(
            status["io_logs"]
                .as_array()
                .is_some_and(|logs| logs.iter().any(|entry| entry.as_str() == Some(log)))
        );
        server.shutdown().await;
        Ok(())
    }
}
