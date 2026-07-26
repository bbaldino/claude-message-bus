PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS agents (
  name         TEXT PRIMARY KEY,
  host         TEXT NOT NULL,
  cwd          TEXT NOT NULL,
  session_id   TEXT,
  connected_at INTEGER NOT NULL,
  last_seen    INTEGER NOT NULL,
  online       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS rooms (
  name       TEXT PRIMARY KEY,
  mode       TEXT NOT NULL DEFAULT 'discuss',
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS room_members (
  room       TEXT NOT NULL REFERENCES rooms(name) ON DELETE CASCADE,
  agent_name TEXT NOT NULL,
  PRIMARY KEY (room, agent_name)
);

CREATE TABLE IF NOT EXISTS messages (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  room       TEXT NOT NULL REFERENCES rooms(name) ON DELETE CASCADE,
  from_agent TEXT NOT NULL,
  body       TEXT NOT NULL,
  done       INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_room_id ON messages(room, id);

CREATE TABLE IF NOT EXISTS files (
  room         TEXT NOT NULL REFERENCES rooms(name) ON DELETE CASCADE,
  key          TEXT NOT NULL,
  sha256       TEXT NOT NULL,
  size         INTEGER NOT NULL,
  content_type TEXT,
  updated_by   TEXT NOT NULL,
  updated_at   INTEGER NOT NULL,
  PRIMARY KEY (room, key)
);

CREATE TABLE IF NOT EXISTS cursors (
  room              TEXT NOT NULL,
  agent_name        TEXT NOT NULL,
  last_delivered_id INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (room, agent_name)
);
