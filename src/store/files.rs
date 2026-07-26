//! Room-scoped artifact storage. Metadata in SQLite, bytes on disk keyed by
//! content hash so identical content is stored once.

use std::fmt::Write as _;

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::{Store, now_ms};

pub const MAX_BLOB_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub room: String,
    pub key: String,
    pub sha256: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub updated_by: String,
    pub updated_at: i64,
}

fn file_row(r: &sqlx::sqlite::SqliteRow) -> FileRow {
    FileRow {
        room: r.get("room"),
        key: r.get("key"),
        sha256: r.get("sha256"),
        size: r.get("size"),
        content_type: r.get("content_type"),
        updated_by: r.get("updated_by"),
        updated_at: r.get("updated_at"),
    }
}

impl Store {
    pub async fn put_file(
        &self,
        room: &str,
        key: &str,
        bytes: &[u8],
        content_type: Option<&str>,
        by: &str,
    ) -> anyhow::Result<FileRow> {
        if bytes.len() > MAX_BLOB_BYTES {
            bail!(
                "file is {:.1} MB; the limit is {:.0} MB",
                bytes.len() as f64 / (1024.0 * 1024.0),
                MAX_BLOB_BYTES as f64 / (1024.0 * 1024.0)
            );
        }
        self.ensure_room(room).await?;

        // sha2's digest output type no longer implements LowerHex, so hex-encode
        // by hand rather than pull in a dedicated hex crate for one call site.
        let digest =
            Sha256::digest(bytes)
                .into_iter()
                .fold(String::with_capacity(64), |mut s, byte| {
                    write!(s, "{byte:02x}").expect("writing to String cannot fail");
                    s
                });
        let path = self.blobs_dir().join(&digest);
        if !path.exists() {
            std::fs::write(&path, bytes).with_context(|| format!("writing blob {digest}"))?;
        }

        let now = now_ms();
        sqlx::query(
            "INSERT INTO files (room, key, sha256, size, content_type, updated_by, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(room, key) DO UPDATE SET
               sha256 = ?3, size = ?4, content_type = ?5, updated_by = ?6, updated_at = ?7",
        )
        .bind(room)
        .bind(key)
        .bind(&digest)
        .bind(bytes.len() as i64)
        .bind(content_type)
        .bind(by)
        .bind(now)
        .execute(self.pool())
        .await?;

        Ok(FileRow {
            room: room.to_string(),
            key: key.to_string(),
            sha256: digest,
            size: bytes.len() as i64,
            content_type: content_type.map(String::from),
            updated_by: by.to_string(),
            updated_at: now,
        })
    }

    pub async fn get_file(
        &self,
        room: &str,
        key: &str,
    ) -> anyhow::Result<Option<(FileRow, Vec<u8>)>> {
        let row = sqlx::query("SELECT * FROM files WHERE room = ?1 AND key = ?2")
            .bind(room)
            .bind(key)
            .fetch_optional(self.pool())
            .await?;
        let Some(row) = row else { return Ok(None) };
        let meta = file_row(&row);
        let bytes = std::fs::read(self.blobs_dir().join(&meta.sha256))
            .with_context(|| format!("blob {} missing from disk", meta.sha256))?;
        Ok(Some((meta, bytes)))
    }

    pub async fn list_files(&self, room: &str) -> anyhow::Result<Vec<FileRow>> {
        let rows = sqlx::query("SELECT * FROM files WHERE room = ?1 ORDER BY key")
            .bind(room)
            .fetch_all(self.pool())
            .await?;
        Ok(rows.iter().map(file_row).collect())
    }
}
