use crate::model::*;
use anyhow::Result;
use rusqlite::{Connection, params};
use std::{path::Path, sync::Mutex};

pub struct Store(pub Mutex<Connection>);
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
            CREATE TABLE IF NOT EXISTS tickets(id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, title TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS prs(id INTEGER PRIMARY KEY, ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE, url TEXT NOT NULL, snapshot TEXT, sync_error TEXT, UNIQUE(ticket_id,url));
            CREATE TABLE IF NOT EXISTS tests(id INTEGER PRIMARY KEY, ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE, label TEXT NOT NULL, passed INTEGER NOT NULL DEFAULT 0, tested_revision TEXT NOT NULL DEFAULT '');
            PRAGMA user_version=1;")?;
        Ok(Self(Mutex::new(conn)))
    }
    pub fn tickets(&self) -> Result<Vec<Ticket>> {
        let db = self.0.lock().unwrap();
        let mut stmt = db.prepare("SELECT id,key,title FROM tickets ORDER BY id DESC")?;
        let tickets = stmt
            .query_map([], |r| {
                Ok(Ticket {
                    id: r.get(0)?,
                    key: r.get(1)?,
                    title: r.get(2)?,
                    prs: vec![],
                    tests: vec![],
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        tickets.into_iter().map(|mut t| {
            let mut prs = db.prepare("SELECT id,url,snapshot,sync_error FROM prs WHERE ticket_id=? ORDER BY id")?;
            let rows = prs.query_map([t.id], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
            for (id,url,snapshot,sync_error) in rows {
                t.prs.push(PullRequest { id, reference: PrRef::parse(&url).map_err(anyhow::Error::msg)?, snapshot: snapshot.map(|s| serde_json::from_str(&s)).transpose()?, sync_error });
            }
            let mut tests = db.prepare("SELECT id,label,passed,tested_revision FROM tests WHERE ticket_id=? ORDER BY id")?;
            t.tests = tests.query_map([t.id], |r| Ok(TestItem { id:r.get(0)?,label:r.get(1)?,passed:r.get(2)?,tested_revision:r.get(3)? }))?.collect::<Result<Vec<_>,_>>()?;
            Ok(t)
        }).collect()
    }
    pub fn add_ticket(&self, key: &str, title: &str) -> Result<i64> {
        let db = self.0.lock().unwrap();
        db.execute(
            "INSERT INTO tickets(key,title) VALUES (?,?)",
            params![key, title],
        )?;
        let id = db.last_insert_rowid();
        for label in [
            "Smoke test on staging",
            "Check the coordinated user journey",
            "Confirm rollback plan",
        ] {
            db.execute(
                "INSERT INTO tests(ticket_id,label) VALUES (?,?)",
                params![id, label],
            )?;
        }
        Ok(id)
    }
    pub fn add_pr(&self, ticket: i64, reference: &PrRef) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO prs(ticket_id,url) VALUES (?,?)",
            params![ticket, reference.url()],
        )?;
        Ok(())
    }
    pub fn save_snapshot(&self, id: i64, snapshot: &Snapshot) -> Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE prs SET snapshot=?,sync_error=NULL WHERE id=?",
            params![serde_json::to_string(snapshot)?, id],
        )?;
        Ok(())
    }
    pub fn fail_sync(&self, id: i64, error: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("UPDATE prs SET sync_error=? WHERE id=?", params![error, id])?;
        Ok(())
    }
    pub fn delete(&self, kind: &str, id: i64) -> Result<bool> {
        let sql = match kind {
            "tickets" => "DELETE FROM tickets WHERE id=?",
            "prs" => "DELETE FROM prs WHERE id=?",
            "tests" => "DELETE FROM tests WHERE id=?",
            _ => anyhow::bail!("Unknown record"),
        };
        Ok(self.0.lock().unwrap().execute(sql, [id])? > 0)
    }
    pub fn add_test(&self, ticket: i64, label: &str) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO tests(ticket_id,label) VALUES (?,?)",
            params![ticket, label],
        )?;
        Ok(())
    }
    pub fn set_test(&self, id: i64, passed: bool) -> Result<bool> {
        let tickets = self.tickets()?;
        let Some(t) = tickets
            .iter()
            .find(|t| t.tests.iter().any(|test| test.id == id))
        else {
            return Ok(false);
        };
        if passed && (t.prs.is_empty() || t.prs.iter().any(|p| !p.fresh())) {
            anyhow::bail!("Sync all linked PRs before recording a pass");
        }
        self.0.lock().unwrap().execute(
            "UPDATE tests SET passed=?,tested_revision=? WHERE id=?",
            params![passed, t.revision(), id],
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_duplicate_and_cascade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radar.sqlite3");
        let db = Store::open(&path).unwrap();
        let id = db.add_ticket("DEMO-1", "Example").unwrap();
        let pr = PrRef::parse("https://github.com/a/b/pull/1").unwrap();
        db.add_pr(id, &pr).unwrap();
        assert!(db.add_pr(id, &pr).is_err());
        drop(db);
        let db = Store::open(&path).unwrap();
        assert_eq!(db.tickets().unwrap()[0].tests.len(), 3);
        assert!(
            db.set_test(db.tickets().unwrap()[0].tests[0].id, true)
                .is_err()
        );
        db.delete("tickets", id).unwrap();
        assert!(db.tickets().unwrap().is_empty());
        assert_eq!(
            db.0.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM prs", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
