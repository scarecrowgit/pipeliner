//! Turning model values into column values and rows back into model values, with errors that
//! name the table, the row and the column.

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use rusqlite::types::{FromSql, Value, ValueRef};
use rusqlite::{Connection, Params};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The serde name of a unit enum variant (`in_progress`), for text columns.
pub(crate) fn tag<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(text) => Ok(text),
        other => bail!("expected a plain enum, got {other}"),
    }
}

/// A value as JSON text, for nested-structure columns.
pub(crate) fn json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

/// A count or token number as SQLite's integer.
pub(crate) fn int(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| anyhow!("{value} does not fit a database integer"))
}

/// Refuses a `~` shortcut: the store keeps paths as the absolute paths they stand for.
pub(crate) fn check_path(path: &Path, what: &str) -> Result<()> {
    if path.to_str().is_some_and(|text| text.starts_with('~')) {
        bail!("{what} {} is a ~ shortcut; expand it before storing", path.display());
    }
    Ok(())
}

/// One row being decoded, with the table and row id for error messages.
pub(crate) struct Decoder<'r, 's> {
    row: &'r rusqlite::Row<'s>,
    table: &'static str,
    id: String,
}

impl<'r, 's> Decoder<'r, 's> {
    /// `id_columns` name the row in errors: `["issue", "seq"]` reads `HAR-1#3`.
    pub(crate) fn new(row: &'r rusqlite::Row<'s>, table: &'static str, id_columns: &[&str]) -> Self {
        let id = id_columns
            .iter()
            .map(|column| match row.get_ref(*column) {
                Ok(ValueRef::Text(text)) => String::from_utf8_lossy(text).into_owned(),
                Ok(ValueRef::Integer(n)) => n.to_string(),
                Ok(other) => format!("{other:?}"),
                Err(_) => "?".to_string(),
            })
            .collect::<Vec<_>>()
            .join("#");
        Self { row, table, id }
    }

    fn context(&self, column: &str) -> String {
        format!("decode {} row {}: column {column}", self.table, self.id)
    }

    pub(crate) fn get<T: FromSql>(&self, column: &str) -> Result<T> {
        self.row.get(column).with_context(|| self.context(column))
    }

    pub(crate) fn string(&self, column: &str) -> Result<String> {
        self.get(column)
    }

    /// A nullable text column as an id type.
    pub(crate) fn opt<T: From<String>>(&self, column: &str) -> Result<Option<T>> {
        Ok(self.get::<Option<String>>(column)?.map(T::from))
    }

    pub(crate) fn u64(&self, column: &str) -> Result<u64> {
        let value: i64 = self.get(column)?;
        u64::try_from(value).with_context(|| self.context(column))
    }

    pub(crate) fn u32(&self, column: &str) -> Result<u32> {
        let value: i64 = self.get(column)?;
        u32::try_from(value).with_context(|| self.context(column))
    }

    pub(crate) fn opt_u32(&self, column: &str) -> Result<Option<u32>> {
        match self.get::<Option<i64>>(column)? {
            Some(value) => Ok(Some(u32::try_from(value).with_context(|| self.context(column))?)),
            None => Ok(None),
        }
    }

    /// A text column holding a unit enum's serde name.
    pub(crate) fn tag<T: DeserializeOwned>(&self, column: &str) -> Result<T> {
        let text = self.string(column)?;
        serde_json::from_value(serde_json::Value::String(text)).with_context(|| self.context(column))
    }

    pub(crate) fn opt_tag<T: DeserializeOwned>(&self, column: &str) -> Result<Option<T>> {
        match self.get::<Option<String>>(column)? {
            Some(text) => {
                Ok(Some(serde_json::from_value(serde_json::Value::String(text)).with_context(|| self.context(column))?))
            }
            None => Ok(None),
        }
    }

    /// A JSON text column.
    pub(crate) fn json<T: DeserializeOwned>(&self, column: &str) -> Result<T> {
        let text = self.string(column)?;
        serde_json::from_str(&text).with_context(|| self.context(column))
    }

    pub(crate) fn opt_json<T: DeserializeOwned>(&self, column: &str) -> Result<Option<T>> {
        match self.get::<Option<String>>(column)? {
            Some(text) => Ok(Some(serde_json::from_str(&text).with_context(|| self.context(column))?)),
            None => Ok(None),
        }
    }
}

/// Runs a query and decodes every row.
pub(crate) fn query_all<T>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
    table: &'static str,
    id_columns: &[&str],
    decode: impl Fn(&Decoder) -> Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(decode(&Decoder::new(row, table, id_columns))?);
    }
    Ok(out)
}

/// Runs a query and decodes its first row, if any.
pub(crate) fn query_one<T>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
    table: &'static str,
    id_columns: &[&str],
    decode: impl Fn(&Decoder) -> Result<T>,
) -> Result<Option<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let mut rows = stmt.query(params)?;
    match rows.next()? {
        Some(row) => Ok(Some(decode(&Decoder::new(row, table, id_columns))?)),
        None => Ok(None),
    }
}

/// A `WHERE` clause built from optional filters, with its parameters in order.
#[derive(Default)]
pub(crate) struct Filter {
    clauses: Vec<String>,
    pub(crate) params: Vec<Value>,
}

impl Filter {
    /// Adds a condition; each `?` in it takes the next of `params`.
    pub(crate) fn and(&mut self, clause: impl Into<String>, params: impl IntoIterator<Item = Value>) {
        self.clauses.push(clause.into());
        self.params.extend(params);
    }

    /// The conditions joined with `AND`, or `1` when there are none.
    pub(crate) fn expr(&self) -> String {
        if self.clauses.is_empty() { "1".to_string() } else { self.clauses.join(" AND ") }
    }
}

/// Text as a parameter value.
pub(crate) fn text(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}
