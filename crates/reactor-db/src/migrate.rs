//! Schema migrations: an ordered list of SQL scripts, the count applied kept in
//! `user_version`. No extra tables, nothing to install, and a database newer than this
//! binary is refused rather than guessed at.

use rusqlite::Connection;

use crate::error::{Error, Result};

const MIGRATIONS: &[&str] = &[
    // 1: authors, blobs, sessions and their append-only log, the change log, and the
    // case's goal and guidelines.
    r#"
    CREATE TABLE author (
      id INTEGER PRIMARY KEY,
      kind TEXT NOT NULL,                 -- analyst|tool|agent
      identity TEXT NOT NULL,
      version TEXT NOT NULL DEFAULT '',
      UNIQUE(kind, identity, version)
    );

    CREATE TABLE blob (
      sha256 TEXT PRIMARY KEY,
      size INTEGER NOT NULL,
      created_at TEXT NOT NULL
    );

    CREATE TABLE session (
      id TEXT PRIMARY KEY,
      title TEXT,
      format_version INTEGER NOT NULL,
      created_at TEXT NOT NULL
    );

    CREATE TABLE entry (
      seq INTEGER PRIMARY KEY,            -- stable rowid, survives VACUUM
      session_id TEXT NOT NULL REFERENCES session(id),
      id INTEGER NOT NULL,                -- dense per session
      parent INTEGER,
      ts INTEGER NOT NULL,                -- unix ms
      type TEXT NOT NULL,
      body TEXT NOT NULL,                 -- the entry as JSON
      blob_sha256 TEXT REFERENCES blob(sha256),
      UNIQUE(session_id, id)
    );
    CREATE TRIGGER entry_no_update BEFORE UPDATE ON entry
      BEGIN SELECT RAISE(ABORT, 'entry is append-only'); END;
    CREATE TRIGGER entry_no_delete BEFORE DELETE ON entry
      BEGIN SELECT RAISE(ABORT, 'entry is append-only'); END;

    CREATE TABLE event (
      seq INTEGER PRIMARY KEY AUTOINCREMENT,
      id TEXT UNIQUE NOT NULL,            -- UUIDv7
      ts TEXT NOT NULL,                   -- RFC 3339 UTC
      session_id TEXT,
      entry_id INTEGER,
      author_id INTEGER REFERENCES author(id),
      type TEXT NOT NULL,
      payload TEXT NOT NULL DEFAULT '{}'
    );
    CREATE TRIGGER event_no_update BEFORE UPDATE ON event
      BEGIN SELECT RAISE(ABORT, 'event is append-only'); END;
    CREATE TRIGGER event_no_delete BEFORE DELETE ON event
      BEGIN SELECT RAISE(ABORT, 'event is append-only'); END;

    CREATE TABLE case_manifest (
      field TEXT NOT NULL,                -- goal|guidelines
      ver INTEGER NOT NULL,
      value TEXT NOT NULL,
      recorded_at TEXT NOT NULL,
      author_id INTEGER NOT NULL REFERENCES author(id),
      event_seq INTEGER REFERENCES event(seq),
      PRIMARY KEY (field, ver)
    );
    CREATE TRIGGER case_manifest_no_update BEFORE UPDATE ON case_manifest
      BEGIN SELECT RAISE(ABORT, 'case_manifest is append-only'); END;
    CREATE TRIGGER case_manifest_no_delete BEFORE DELETE ON case_manifest
      BEGIN SELECT RAISE(ABORT, 'case_manifest is append-only'); END;
    "#,
    // 2: the analysis graph (ADR-0047), its vocabulary, and the registry of referenced
    // files (ADR-0045).
    r#"
    CREATE TABLE element (
      id TEXT PRIMARY KEY,
      kind TEXT NOT NULL CHECK (kind IN ('entity','event','observation','annotation','external')),
      type TEXT NOT NULL
    );
    CREATE TRIGGER element_no_update BEFORE UPDATE ON element
      BEGIN SELECT RAISE(ABORT, 'an element''s kind and type never change'); END;

    CREATE TABLE element_version (
      element_id TEXT NOT NULL REFERENCES element(id),
      ver INTEGER NOT NULL,
      recorded_at TEXT NOT NULL,          -- record time: when this version was written
      author_id INTEGER NOT NULL REFERENCES author(id),
      on_behalf_of INTEGER REFERENCES author(id),
      event_seq INTEGER REFERENCES event(seq),
      status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active','conflicting','soft-deleted')),
      confidence TEXT,
      value TEXT,
      props TEXT NOT NULL DEFAULT '{}',
      tags TEXT NOT NULL DEFAULT '[]',
      -- domain time: when it happened or held. Never record time.
      occurred_at TEXT, duration_ms INTEGER, valid_from TEXT, valid_to TEXT,
      tz TEXT, tz_certainty TEXT, time_raw TEXT, time_precision TEXT,
      PRIMARY KEY (element_id, ver)
    );
    CREATE INDEX element_version_event ON element_version(event_seq);
    CREATE TRIGGER element_version_no_update BEFORE UPDATE ON element_version
      BEGIN SELECT RAISE(ABORT, 'a version is never overwritten'); END;

    CREATE TABLE edge (
      id TEXT PRIMARY KEY,
      from_id TEXT NOT NULL REFERENCES element(id),
      to_id TEXT NOT NULL REFERENCES element(id)
    );
    CREATE INDEX edge_from ON edge(from_id);
    CREATE INDEX edge_to ON edge(to_id);
    CREATE TRIGGER edge_no_update BEFORE UPDATE ON edge
      BEGIN SELECT RAISE(ABORT, 'an edge''s ends never change'); END;
    -- Claims are nodes: nothing is said by an edge between two entities, an entity
    -- points only at where it came from, and an external points at nothing.
    CREATE TRIGGER edge_shape BEFORE INSERT ON edge
      BEGIN
        SELECT RAISE(ABORT, 'no edges between entities: a relation is an event or an observation')
          WHERE (SELECT kind FROM element WHERE id = NEW.from_id) = 'entity'
            AND (SELECT kind FROM element WHERE id = NEW.to_id) = 'entity';
        SELECT RAISE(ABORT, 'an entity''s only outgoing edge is to its source')
          WHERE (SELECT kind FROM element WHERE id = NEW.from_id) = 'entity'
            AND (SELECT kind FROM element WHERE id = NEW.to_id) <> 'external';
        SELECT RAISE(ABORT, 'an external has no outgoing edges')
          WHERE (SELECT kind FROM element WHERE id = NEW.from_id) = 'external';
      END;

    CREATE TABLE edge_version (
      edge_id TEXT NOT NULL REFERENCES edge(id),
      ver INTEGER NOT NULL,
      role TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','removed')),
      pinned_ver INTEGER,                 -- the version of the target this edge judged
      locator TEXT,
      fingerprint TEXT,
      recorded_at TEXT NOT NULL,
      author_id INTEGER NOT NULL REFERENCES author(id),
      event_seq INTEGER REFERENCES event(seq),
      PRIMARY KEY (edge_id, ver)
    );
    CREATE INDEX edge_version_event ON edge_version(event_seq);
    CREATE TRIGGER edge_version_no_update BEFORE UPDATE ON edge_version
      BEGIN SELECT RAISE(ABORT, 'a version is never overwritten'); END;
    CREATE TRIGGER edge_version_shape BEFORE INSERT ON edge_version
      BEGIN
        SELECT RAISE(ABORT, 'an entity''s only outgoing edge is to its source')
          WHERE NEW.role <> 'source'
            AND (SELECT e.kind FROM edge g JOIN element e ON e.id = g.from_id
                  WHERE g.id = NEW.edge_id) = 'entity';
        SELECT RAISE(ABORT, 'a source is an external')
          WHERE NEW.role = 'source'
            AND (SELECT e.kind FROM edge g JOIN element e ON e.id = g.to_id
                  WHERE g.id = NEW.edge_id) <> 'external';
      END;

    CREATE VIEW element_current AS
      SELECT e.id, e.kind, e.type, v.ver, v.recorded_at, v.author_id, v.on_behalf_of,
             v.event_seq, v.status, v.confidence, v.value, v.props, v.tags,
             v.occurred_at, v.duration_ms, v.valid_from, v.valid_to,
             v.tz, v.tz_certainty, v.time_raw, v.time_precision
        FROM element e JOIN element_version v ON v.element_id = e.id
       WHERE v.ver = (SELECT max(ver) FROM element_version WHERE element_id = e.id);

    -- Edges as they stand: the latest version, where it has not been removed.
    CREATE VIEW edge_current AS
      SELECT g.id, g.from_id, g.to_id, v.ver, v.role, v.pinned_ver, v.locator,
             v.fingerprint, v.recorded_at, v.author_id, v.event_seq
        FROM edge g JOIN edge_version v ON v.edge_id = g.id
       WHERE v.ver = (SELECT max(ver) FROM edge_version WHERE edge_id = g.id)
         AND v.status = 'active';

    -- The vocabulary: conventions, kept as data so they grow without a release.
    CREATE TABLE known_type (
      name TEXT PRIMARY KEY,
      kind TEXT NOT NULL,
      definition TEXT NOT NULL DEFAULT '',
      normalizer TEXT,                    -- a built-in function's name
      key_fields TEXT NOT NULL DEFAULT '["value"]',
      stale_after_days INTEGER
    );
    CREATE TABLE type_alias (alias TEXT PRIMARY KEY, name TEXT NOT NULL);
    CREATE TABLE known_role (
      name TEXT PRIMARY KEY,
      definition TEXT NOT NULL DEFAULT '',
      pair TEXT,                          -- the role at the other end, where there is one
      direction TEXT CHECK (direction IN ('from','to'))
    );
    CREATE TABLE scale_value (
      scale TEXT NOT NULL,                -- confidence|reliability
      ord INTEGER NOT NULL,
      name TEXT NOT NULL,
      definition TEXT NOT NULL DEFAULT '',
      PRIMARY KEY (scale, ord), UNIQUE (scale, name)
    );

    -- A binary claim as one row: what a person means by "A is related to B".
    CREATE VIEW relations AS
      WITH part AS (
        SELECT g.from_id AS claim_id, g.to_id, g.role
          FROM edge_current g JOIN element t ON t.id = g.to_id
         WHERE g.role <> 'source' AND t.kind <> 'external'
      )
      SELECT a.to_id AS from_id, b.to_id AS to_id, c.type, c.id AS claim_id,
             a.role AS from_role, b.role AS to_role
        FROM element_current c
        JOIN part a ON a.claim_id = c.id
        JOIN part b ON b.claim_id = c.id AND (a.to_id <> b.to_id OR a.role <> b.role)
        LEFT JOIN known_role ra ON ra.name = a.role
        LEFT JOIN known_role rb ON rb.name = b.role
       WHERE c.kind IN ('event','observation') AND c.status <> 'soft-deleted'
         AND (SELECT count(*) FROM part p WHERE p.claim_id = c.id) = 2
         AND CASE
               WHEN ra.direction = 'from' OR rb.direction = 'to' THEN 1
               WHEN ra.direction = 'to' OR rb.direction = 'from' THEN 0
               ELSE a.role < b.role OR (a.role = b.role AND a.to_id < b.to_id)
             END;

    CREATE TABLE artifact (
      id TEXT PRIMARY KEY,                -- UUIDv7
      path TEXT NOT NULL UNIQUE,          -- relative to the case root, forward slashes
      sha256 TEXT NOT NULL,
      sha1 TEXT, md5 TEXT,
      size INTEGER NOT NULL,
      mime TEXT,
      kind TEXT,
      parent_id TEXT REFERENCES artifact(id),
      attrs TEXT NOT NULL DEFAULT '{}',
      registered_at TEXT NOT NULL,
      event_seq INTEGER REFERENCES event(seq)
    );

    INSERT INTO known_type(name, kind, definition, normalizer) VALUES
      ('net.ipv4', 'entity', 'An IPv4 address.', 'ip'),
      ('net.ipv6', 'entity', 'An IPv6 address.', 'ip'),
      ('net.domain', 'entity', 'A DNS name, without a trailing dot.', 'domain'),
      ('net.url', 'entity', 'A URL as seen.', 'trim'),
      ('net.email', 'entity', 'An email address.', 'lowercase'),
      ('file.hash.md5', 'entity', 'An MD5 digest of a file, in hex.', 'lowercase'),
      ('file.hash.sha1', 'entity', 'A SHA-1 digest of a file, in hex.', 'lowercase'),
      ('file.hash.sha256', 'entity', 'A SHA-256 digest of a file, in hex.', 'lowercase'),
      ('file.name', 'entity', 'A file name without its directory.', 'trim'),
      ('file.path', 'entity', 'A path to a file on some host.', 'trim'),
      ('host.name', 'entity', 'The name a host is known by.', 'lowercase'),
      ('account.user', 'entity', 'A user account name.', 'trim'),
      ('process.name', 'entity', 'An executable''s name as a process.', 'trim'),
      ('registry.key', 'entity', 'A Windows registry key path.', 'trim'),
      ('analysis.hypothesis', 'annotation', 'A candidate explanation; receives supports and refutes.', NULL),
      ('analysis.judgement', 'annotation', 'An assessment of a claim, with a confidence.', NULL),
      ('analysis.note', 'annotation', 'Commentary, a question or a to-do.', NULL),
      ('analysis.cluster', 'annotation', 'A claim that its members belong together.', NULL),
      ('analysis.same_as', 'annotation', 'A decision that two entities are the same thing.', NULL),
      ('analysis.search_negative', 'annotation', 'A search that found nothing, and where it looked.', NULL),
      ('analysis.pull', 'event', 'Rows of an external were pulled into the graph.', NULL);
    INSERT INTO known_type(name, kind, definition, normalizer, key_fields) VALUES
      ('process.instance', 'entity', 'One run of a process: a host, a pid and a start time.',
       NULL, '["host","pid","start"]');
    INSERT INTO type_alias(alias, name) VALUES
      ('ipv4', 'net.ipv4'), ('ip', 'net.ipv4'), ('net.ip', 'net.ipv4'), ('net.ip4', 'net.ipv4'),
      ('ipv6', 'net.ipv6'), ('net.ip6', 'net.ipv6'),
      ('domain', 'net.domain'), ('hostname', 'host.name'), ('host', 'host.name'),
      ('url', 'net.url'), ('email', 'net.email'),
      ('md5', 'file.hash.md5'), ('sha1', 'file.hash.sha1'), ('sha256', 'file.hash.sha256'),
      ('user', 'account.user'), ('account', 'account.user');
    INSERT INTO known_role(name, definition, pair, direction) VALUES
      ('src', 'Where an occurrence came from.', 'dst', 'from'),
      ('dst', 'Where an occurrence went to.', 'src', 'to'),
      ('actor', 'Who or what did it.', 'target', 'from'),
      ('target', 'What it was done to.', 'actor', 'to'),
      ('parent', 'The one that started or contains the other.', 'child', 'from'),
      ('child', 'The one started or contained.', 'parent', 'to'),
      ('subject', 'What a state is about.', 'object', 'from'),
      ('object', 'What the subject is related to.', 'subject', 'to'),
      ('query', 'What was asked.', 'answer', 'from'),
      ('answer', 'What came back.', 'query', 'to'),
      ('cause', 'What brought the other about.', 'effect', 'from'),
      ('effect', 'What was brought about.', 'cause', 'to');
    INSERT INTO known_role(name, definition) VALUES
      ('basis', 'A claim this one was derived from.'),
      ('about', 'What an annotation comments on.'),
      ('supports', 'Evidence for a hypothesis.'),
      ('refutes', 'Evidence against a hypothesis.'),
      ('member', 'A member of a cluster.'),
      ('candidate', 'One of several possibilities; uncertain, and left out of projections.'),
      ('source', 'The external a claim comes from.');
    INSERT INTO scale_value(scale, ord, name) VALUES
      ('confidence', 1, 'almost no chance'), ('confidence', 2, 'very unlikely'),
      ('confidence', 3, 'unlikely'), ('confidence', 4, 'roughly even'),
      ('confidence', 5, 'likely'), ('confidence', 6, 'very likely'),
      ('confidence', 7, 'almost certain'),
      ('reliability', 1, 'A'), ('reliability', 2, 'B'), ('reliability', 3, 'C'),
      ('reliability', 4, 'D'), ('reliability', 5, 'E'), ('reliability', 6, 'F');
    "#,
    // 3: the context corpus (ADR-0046): documents and their chunks, a keyword index over
    // the chunks, and the bookkeeping for vectors. The vectors themselves live in one
    // `chunk_vec_<model id>` table per embedding model, created when the model is.
    r#"
    CREATE TABLE document (
      id TEXT PRIMARY KEY,
      source_uri TEXT NOT NULL,
      source_kind TEXT NOT NULL,          -- file|url|wiki|import
      content_sha256 TEXT NOT NULL REFERENCES blob(sha256),
      normalized_md_sha256 TEXT REFERENCES blob(sha256),
      mime TEXT,
      title TEXT,
      origin TEXT NOT NULL DEFAULT 'external',   -- external|analyst|sample
      fetched_at TEXT,
      added_at TEXT NOT NULL,
      meta TEXT NOT NULL DEFAULT '{}',
      UNIQUE(source_uri, content_sha256)
    );
    CREATE INDEX document_source ON document(source_uri);

    CREATE TABLE chunk (
      id INTEGER PRIMARY KEY,
      document_id TEXT NOT NULL REFERENCES document(id) ON DELETE CASCADE,
      ord INTEGER NOT NULL,
      heading_path TEXT NOT NULL DEFAULT '',
      text TEXT NOT NULL,
      text_sha256 TEXT NOT NULL,
      chunker TEXT NOT NULL,
      UNIQUE(document_id, ord)
    );

    CREATE VIRTUAL TABLE chunk_fts USING fts5(
      text, heading_path, content='chunk', content_rowid='id', tokenize='unicode61'
    );
    CREATE TRIGGER chunk_ai AFTER INSERT ON chunk BEGIN
      INSERT INTO chunk_fts(rowid, text, heading_path) VALUES (new.id, new.text, new.heading_path);
    END;
    CREATE TRIGGER chunk_ad AFTER DELETE ON chunk BEGIN
      INSERT INTO chunk_fts(chunk_fts, rowid, text, heading_path)
        VALUES ('delete', old.id, old.text, old.heading_path);
    END;
    CREATE TRIGGER chunk_au AFTER UPDATE ON chunk BEGIN
      INSERT INTO chunk_fts(chunk_fts, rowid, text, heading_path)
        VALUES ('delete', old.id, old.text, old.heading_path);
      INSERT INTO chunk_fts(rowid, text, heading_path) VALUES (new.id, new.text, new.heading_path);
    END;

    CREATE TABLE embedding_model (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,                 -- provider/model
      revision TEXT NOT NULL DEFAULT '',
      dim INTEGER NOT NULL,
      created_at TEXT NOT NULL,
      UNIQUE(name, revision)
    );
    CREATE TABLE embedding_status (
      chunk_id INTEGER NOT NULL REFERENCES chunk(id) ON DELETE CASCADE,
      model_id INTEGER NOT NULL REFERENCES embedding_model(id) ON DELETE CASCADE,
      embedded_at TEXT NOT NULL,
      PRIMARY KEY (chunk_id, model_id)
    );
    CREATE INDEX embedding_status_model ON embedding_status(model_id);
    "#,
    // 4: where the graph's nodes were left in the drawn view. Presentation, not analysis: no
    // versions, no change log, and a node that is gone leaves its row behind harmlessly.
    r#"
    CREATE TABLE graph_layout (
      element_id TEXT PRIMARY KEY,
      x REAL NOT NULL,
      y REAL NOT NULL,
      updated_at TEXT NOT NULL
    );
    "#,
];

/// The schema version this binary writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// Bring `conn` up to [`SCHEMA_VERSION`]. Each migration is one transaction.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let have: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if have > SCHEMA_VERSION {
        return Err(Error::Build(format!(
            "this database is at schema {have}; this build of REactor knows up to {SCHEMA_VERSION}"
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(have as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
