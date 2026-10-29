use crate::domain::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};
use uuid::Uuid;

pub struct Library {
    db: Connection,
}
impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            version <= 1,
            "Banco de uma versão mais nova do WGR; preserve os dados e atualize o app."
        );
        if version == 0 {
            if db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get::<_, u32>(0),
            )? > 0
            {
                db.backup("main", path.with_extension("pre-v1.sqlite"), None)?;
            }
            db.execute_batch(include_str!("../../../migrations/001_library.sql"))?;
        }
        Ok(Self { db })
    }
    fn get_json<T: DeserializeOwned>(&self, sql: &str, id: &str) -> Result<Option<T>> {
        let text: Option<String> = self.db.query_row(sql, [id], |r| r.get(0)).optional()?;
        text.map(|v| serde_json::from_str(&v).context("Registro local inválido"))
            .transpose()
    }
    pub fn sources(&self) -> Result<Vec<Source>> {
        self.list("SELECT json FROM sources ORDER BY position,created_at")
    }
    pub fn games(&self) -> Result<Vec<Game>> {
        self.list("SELECT json FROM games ORDER BY position,created_at")
    }
    fn list<T: DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>> {
        self.db
            .prepare(sql)?
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn game(&self, id: Uuid) -> Result<Option<Game>> {
        self.get_json("SELECT json FROM games WHERE id=?1", &id.to_string())
    }
    pub fn source(&self, id: Uuid) -> Result<Option<Source>> {
        self.get_json("SELECT json FROM sources WHERE id=?1", &id.to_string())
    }
    pub fn save_source(&self, source: &Source) -> Result<()> {
        save_source(&self.db, source)
    }
    pub fn save_game(&self, game: &Game) -> Result<()> {
        save_game(&self.db, game)
    }
    pub fn import(&mut self, source: &Source, games: &[Game]) -> Result<usize> {
        let tx = self.db.transaction()?;
        save_source(&tx, source)?;
        for incoming in games {
            let mut game = incoming.clone();
            if let Some(ext) = &game.external_id {
                let old: Option<String> = tx
                    .query_row(
                        "SELECT json FROM games WHERE source_id=?1 AND external_id=?2",
                        params![source.id.to_string(), ext],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(old) = old {
                    let previous: Game = serde_json::from_str(&old)?;
                    game.id = previous.id;
                    game.favorite = previous.favorite;
                    game.play_count = previous.play_count;
                    game.last_played_at = previous.last_played_at;
                    game.created_at = previous.created_at;
                    game.profile = previous.profile;
                }
            }
            save_game(&tx, &game)?;
        }
        tx.commit()?;
        Ok(games.len())
    }
    pub fn remove_game(&self, id: Uuid) -> Result<()> {
        self.db
            .execute("DELETE FROM games WHERE id=?1", [id.to_string()])?;
        Ok(())
    }
    pub fn remove_source(&self, id: Uuid) -> Result<()> {
        self.db
            .execute("DELETE FROM sources WHERE id=?1", [id.to_string()])?;
        Ok(())
    }
    pub fn reorder(&mut self, source: bool, ids: &[Uuid]) -> Result<()> {
        let table = if source { "sources" } else { "games" };
        let current: Vec<String> = self
            .db
            .prepare(&format!("SELECT id FROM {table}"))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let unique: std::collections::HashSet<_> = ids.iter().map(ToString::to_string).collect();
        ensure!(
            unique.len() == ids.len()
                && current.len() == ids.len()
                && current.iter().all(|id| unique.contains(id)),
            "Ordem incompleta ou duplicada"
        );
        let tx = self.db.transaction()?;
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                &format!("UPDATE {table} SET position=?1 WHERE id=?2"),
                params![position as i64, id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn settings(&self) -> Result<Settings> {
        Ok(self
            .get_json("SELECT json FROM settings WHERE key=?1", "global")?
            .unwrap_or_default())
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.set_value("global", settings)
    }
    pub fn set_value<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.db.execute("INSERT INTO settings(key,json) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",params![key,serde_json::to_string(value)?])?;
        Ok(())
    }
    pub fn value<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.get_json("SELECT json FROM settings WHERE key=?1", key)
    }
    pub fn save_session(&mut self, session: &Session) -> Result<()> {
        let tx = self.db.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            [session.id.to_string()],
            |r| r.get(0),
        )?;
        tx.execute("INSERT INTO sessions(id,game_id,json) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
            params![session.id.to_string(),session.game_id.to_string(),serde_json::to_string(session)?])?;
        if !exists {
            let text: Option<String> = tx
                .query_row(
                    "SELECT json FROM games WHERE id=?1",
                    [session.game_id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(text) = text {
                let mut game: Game = serde_json::from_str(&text)?;
                game.play_count += 1;
                game.last_played_at = Some(session.start_at);
                save_game(&tx, &game)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn sessions(&self) -> Result<Vec<Session>> {
        self.list("SELECT json FROM sessions ORDER BY rowid DESC LIMIT 100")
    }
    pub fn recover_sessions(&mut self) -> Result<()> {
        for mut s in self.sessions()? {
            if s.end_at.is_none() {
                s.state = SessionState::Crashed;
                s.end_at = Some(now());
                s.reason = Some("Sessão interrompida na execução anterior".into());
                self.save_session(&s)?;
            }
        }
        Ok(())
    }
}
fn save_source(db: &Connection, source: &Source) -> Result<()> {
    ensure!(
        !source.display_name.trim().is_empty() && source.display_name.len() <= 256,
        "Nome de fonte inválido"
    );
    db.execute("INSERT INTO sources(id,kind,json,created_at,position) VALUES(?1,?2,?3,?4,(SELECT COALESCE(MAX(position),-1)+1 FROM sources)) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,json=excluded.json",
        params![source.id.to_string(),serde_json::to_string(&source.kind)?,serde_json::to_string(source)?,source.created_at])?;
    Ok(())
}
fn save_game(db: &Connection, game: &Game) -> Result<()> {
    ensure!(
        !game.title.trim().is_empty() && game.title.len() <= 256 && game.entry_uri.len() <= 4096,
        "Metadados do jogo inválidos"
    );
    db.execute("INSERT INTO games(id,source_id,external_id,title,favorite,created_at,position,json) VALUES(?1,?2,?3,?4,?5,?6,(SELECT COALESCE(MAX(position),-1)+1 FROM games),?7) ON CONFLICT(id) DO UPDATE SET title=excluded.title,favorite=excluded.favorite,json=excluded.json",
        params![game.id.to_string(),game.source_id.to_string(),game.external_id,game.title,game.favorite,game.created_at,serde_json::to_string(game)?])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_is_atomic_idempotent_and_survives_restart() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("library.sqlite");
        let mut db = Library::open(&path)?;
        let source = Source::new(SourceKind::RemoteCatalogApi, "Teste", "https://example.com");
        let mut game = Game::new(&source, "Um", "https://example.com/play");
        game.external_id = Some("one".into());
        db.import(&source, &[game.clone()])?;
        game.favorite = true;
        db.save_game(&game)?;
        let mut update = game.clone();
        update.id = Uuid::new_v4();
        update.favorite = false;
        update.title = "Atualizado".into();
        db.import(&source, &[update])?;
        let bad = Game::new(&source, "", "bad");
        assert!(db.import(&source, &[bad]).is_err());
        drop(db);
        let db = Library::open(&path)?;
        let games = db.games()?;
        assert_eq!(games.len(), 1);
        assert!(games[0].favorite);
        assert_eq!(games[0].id, game.id);
        db.remove_source(source.id)?;
        assert!(db.games()?.is_empty());
        Ok(())
    }
    #[test]
    fn reorder_checks_full_permutation() -> Result<()> {
        let t = tempfile::tempdir()?;
        let mut db = Library::open(&t.path().join("x.db"))?;
        let s = Source::new(SourceKind::RemoteWeb, "x", "https://example.com");
        let a = Game::new(&s, "A", "https://example.com/a");
        let b = Game::new(&s, "B", "https://example.com/b");
        db.import(&s, &[a.clone(), b.clone()])?;
        assert!(db.reorder(false, &[a.id, a.id]).is_err());
        db.reorder(false, &[b.id, a.id])?;
        assert_eq!(db.games()?[0].id, b.id);
        Ok(())
    }
}
