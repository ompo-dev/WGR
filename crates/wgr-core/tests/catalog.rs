use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
};
use serde_json::json;
use std::collections::HashMap;
use wgr_core::{
    catalog,
    domain::{Source, SourceKind},
    security::{HttpClient, NetworkPolicy},
    storage::Library,
};

#[tokio::test]
async fn two_page_sync_is_idempotent_and_rejects_redirect_ssrf() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let router=Router::new().route("/.well-known/wgr.json",get(|State(origin):State<String>|async move{Json(json!({"schema_version":"1.0","publisher":"Fixture livre","catalog_url":format!("{origin}/wgr/v1/games"),"allowed_asset_origins":[]}))}))
        .route("/wgr/v1/games",get(|State(origin):State<String>,Query(query):Query<HashMap<String,String>>|async move{let second=query.contains_key("cursor");Json(json!({"schema_version":"1.0","items":[{"id":if second{"two"}else{"one"},"title":if second{"Segundo"}else{"Primeiro"},"entry_url":format!("{origin}/game.html"),"runtime":"webview2","requirements":{}}],"next_cursor":if second{None}else{Some("cursor-2")}}))}))
        .route("/redirect",get(||async{(axum::http::StatusCode::FOUND,[("location","https://169.254.169.254/latest/meta-data/")]).into_response()})).with_state(origin.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.expect("fixture") });
    let mut source = Source::new(SourceKind::RemoteCatalogApi, "Fixture", &origin);
    source.development_loopback = true;
    source.allowed_origins = vec![origin.clone()];
    let (source, games) = catalog::sync(&source, None).await?;
    assert_eq!(games.len(), 2);
    let t = tempfile::tempdir()?;
    let mut library = Library::open(&t.path().join("library.sqlite"))?;
    library.import(&source, &games)?;
    let ids: Vec<_> = library.games()?.iter().map(|g| g.id).collect();
    let (source, games) = catalog::sync(&source, None).await?;
    library.import(&source, &games)?;
    assert_eq!(
        library.games()?.iter().map(|g| g.id).collect::<Vec<_>>(),
        ids
    );
    let http = HttpClient::new(NetworkPolicy::for_url(&origin, true)?);
    assert!(http.get(&format!("{origin}/redirect"), 4096).await.is_err());
    task.abort();
    Ok(())
}
