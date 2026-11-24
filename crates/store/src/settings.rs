//! Projects, app settings and one-off markers ("v1 import done").

use anyhow::{Result, bail};
use model::{Project, ProjectId, Settings};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::Store;
use crate::codec::{Decoder, check_path, json, query_all, query_one};

fn decode_project(d: &Decoder) -> Result<Project> {
    Ok(Project {
        id: ProjectId(d.string("id")?),
        name: d.string("name")?,
        key_prefix: d.string("key_prefix")?,
        repos: d.json("repos")?,
    })
}

/// Sets a marker inside the caller's transaction.
pub(crate) fn set_marker_in(tx: &Transaction, key: &str, value: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

impl Store {
    /// Creates a project or replaces it.
    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        if project.key_prefix.is_empty() {
            bail!("project {} needs an issue key prefix", project.id);
        }
        for repo in &project.repos {
            check_path(repo, "project repo")?;
        }
        let repos = json(&project.repos)?;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO projects (id, name, key_prefix, repos) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name, key_prefix = excluded.key_prefix,
                    repos = excluded.repos",
                params![project.id.as_str(), project.name, project.key_prefix, repos],
            )?;
            Ok(())
        })
    }

    /// One project.
    pub fn project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.read(|conn| {
            query_one(
                conn,
                "SELECT id, name, key_prefix, repos FROM projects WHERE id = ?1",
                [id.as_str()],
                "projects",
                &["id"],
                decode_project,
            )
        })
    }

    /// Every project, by name.
    pub fn projects(&self) -> Result<Vec<Project>> {
        self.read(|conn| {
            query_all(
                conn,
                "SELECT id, name, key_prefix, repos FROM projects ORDER BY name, id",
                [],
                "projects",
                &["id"],
                decode_project,
            )
        })
    }

    /// Deletes a project. Its issues stay. Returns whether it existed.
    pub fn delete_project(&self, id: &ProjectId) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM projects WHERE id = ?1", [id.as_str()])? > 0))
    }

    /// The app settings; defaults until they are first set.
    pub fn settings(&self) -> Result<Settings> {
        let found = self.read(|conn| {
            query_one(conn, "SELECT id, data FROM settings WHERE id = 1", [], "settings", &["id"], |d| d.json("data"))
        })?;
        Ok(found.unwrap_or_default())
    }

    /// Replaces the app settings.
    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        let data = json(settings)?;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO settings (id, data) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET data = excluded.data",
                [data],
            )?;
            Ok(())
        })
    }

    /// A one-off marker, such as `v1_import_done`.
    pub fn marker(&self, key: &str) -> Result<Option<String>> {
        self.read(|conn| {
            Ok(conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0)).optional()?)
        })
    }

    /// Sets a one-off marker.
    pub fn set_marker(&self, key: &str, value: &str) -> Result<()> {
        self.write(|tx| set_marker_in(tx, key, value))
    }

    /// Every marker whose key starts with `prefix`, with its value, in key order.
    pub fn markers_with_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        self.read(|conn| {
            let mut stmt = conn.prepare_cached("SELECT key, value FROM meta WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key")?;
            let rows = stmt.query_map([prefix], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Removes a marker. Returns whether it was set.
    pub fn remove_marker(&self, key: &str) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM meta WHERE key = ?1", [key])? > 0))
    }
}
