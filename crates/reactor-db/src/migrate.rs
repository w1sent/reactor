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
    // 5: how a type is drawn, and a wider vocabulary. A type may name an icon (a Lucide
    // icon's name) and a colour (one of `graph::vocab::COLOURS`); a *namespace* -- a dotted
    // prefix, `net` or `file.hash` -- says what its types have in common and how they are
    // drawn when they say nothing themselves. Presentation, like the layout: no versions.
    // A node may also be drawn at a size of its own, or a width set by hand.
    r#"
    ALTER TABLE known_type ADD COLUMN icon TEXT;
    ALTER TABLE known_type ADD COLUMN colour TEXT;
    CREATE TABLE type_namespace (
      prefix TEXT PRIMARY KEY,            -- `net`, `file.hash`
      definition TEXT NOT NULL DEFAULT '',
      icon TEXT,
      colour TEXT
    );
    CREATE TABLE graph_node_size (
      element_id TEXT PRIMARY KEY,
      size TEXT,                          -- s|m|l|xl, when not the view's
      width REAL                          -- world units, when set by hand
    );

    INSERT INTO type_namespace(prefix, definition, icon, colour) VALUES
      ('net', 'Network: addresses, names and traffic.', 'network', 'blue'),
      ('net.tls', 'TLS: certificates and fingerprints.', 'globe-lock', 'blue'),
      ('mail', 'Email as messages.', 'mail', 'sky'),
      ('cloud', 'Cloud accounts and resources.', 'cloud', 'cobalt'),
      ('host', 'Machines and what is configured on them.', 'monitor', 'teal'),
      ('device', 'Phones, tablets and peripherals.', 'smartphone', 'slate'),
      ('app', 'Applications as installed.', 'app-window', 'mint'),
      ('account', 'Accounts, groups and credentials.', 'user', 'lime'),
      ('auth', 'Authentication: logons and privileges.', 'log-in', 'green'),
      ('person', 'People.', 'contact', 'sand'),
      ('org', 'Organisations.', 'building-2', 'sand'),
      ('geo', 'Places.', 'map-pin', 'slate'),
      ('doc', 'Reports and references.', 'newspaper', 'grey'),
      ('file', 'Files: paths, names and content.', 'file', 'purple'),
      ('file.hash', 'Digests of a file.', 'hash', 'purple'),
      ('process', 'Processes and what they load.', 'square-terminal', 'pink'),
      ('registry', 'The Windows registry.', 'key-round', 'brown'),
      ('mem', 'Memory forensics.', 'memory-stick', 'cyan'),
      ('evidence', 'Evidence and its custody.', 'hard-drive', 'brown'),
      ('threat', 'Threat intelligence: who, with what, against whom.', 'swords', 'magenta'),
      ('attack', 'MITRE ATT&CK.', 'crosshair', 'indigo'),
      ('detect', 'Detection: rules and what they fired on.', 'radar', 'olive'),
      ('vuln', 'Vulnerabilities and exploits.', 'shield-alert', 'yellow'),
      ('malware', 'Malware analysis: families, samples, behaviour.', 'bug', 'violet'),
      ('code', 'Reverse engineering: what is inside a binary.', 'binary', 'cyan'),
      ('crypto', 'Cryptography: algorithms, keys, certificates.', 'lock-keyhole', 'yellow'),
      ('software', 'Software, versions and packages.', 'package', 'mint'),
      ('misc', 'Anything that has no better home.', 'box', 'grey'),
      ('analysis', 'What was concluded.', 'sticky-note', 'cream');

    UPDATE known_type SET icon = 'globe' WHERE name = 'net.domain';
    UPDATE known_type SET icon = 'link' WHERE name = 'net.url';
    UPDATE known_type SET icon = 'at-sign' WHERE name = 'net.email';
    UPDATE known_type SET icon = 'file-text' WHERE name = 'file.name';
    UPDATE known_type SET icon = 'cpu' WHERE name = 'process.instance';
    UPDATE known_type SET icon = 'lightbulb' WHERE name = 'analysis.hypothesis';
    UPDATE known_type SET icon = 'scale' WHERE name = 'analysis.judgement';
    UPDATE known_type SET icon = 'group' WHERE name = 'analysis.cluster';
    UPDATE known_type SET icon = 'link-2' WHERE name = 'analysis.same_as';
    UPDATE known_type SET icon = 'search-x' WHERE name = 'analysis.search_negative';
    UPDATE known_type SET icon = 'database' WHERE name = 'analysis.pull';

    INSERT OR IGNORE INTO known_type(name, kind, definition, normalizer, icon) VALUES
      -- net
      ('net.cidr', 'entity', 'An address range in CIDR notation.', 'trim', 'waypoints'),
      ('net.asn', 'entity', 'An autonomous system, as AS64500.', 'uppercase', 'route'),
      ('net.port', 'entity', 'A port and protocol, as 443/tcp.', 'lowercase', 'ethernet-port'),
      ('net.socket', 'entity', 'An address and port, as 198.51.100.7:443.', 'lowercase', 'ethernet-port'),
      ('net.mac', 'entity', 'A hardware address.', 'lowercase', 'cable'),
      ('net.user_agent', 'entity', 'A User-Agent string as sent.', 'trim', 'text-quote'),
      ('net.tls.cert', 'entity', 'A TLS certificate, by its SHA-256 fingerprint.', 'lowercase', 'file-badge'),
      ('net.tls.sni', 'entity', 'A server name asked for in a TLS handshake.', 'domain', NULL),
      ('net.tls.ja3', 'entity', 'A JA3 client fingerprint.', 'lowercase', 'fingerprint-pattern'),
      ('net.tls.ja3s', 'entity', 'A JA3S server fingerprint.', 'lowercase', 'fingerprint-pattern'),
      ('net.tls.jarm', 'entity', 'A JARM server fingerprint.', 'lowercase', 'fingerprint-pattern'),
      ('net.connection', 'event', 'One endpoint connected to another.', NULL, NULL),
      ('net.flow', 'event', 'Traffic between two endpoints, summarised.', NULL, NULL),
      ('net.dns.query', 'event', 'A name was asked for.', NULL, NULL),
      ('net.http.request', 'event', 'An HTTP request was made.', NULL, NULL),
      ('net.resolution', 'observation', 'A name resolved to an address.', NULL, NULL),
      ('net.listens', 'observation', 'A host listens on a port.', NULL, NULL),
      -- mail
      ('mail.message', 'entity', 'An email, by its Message-ID.', 'trim', NULL),
      ('mail.subject', 'entity', 'A subject line as written.', 'trim', 'text-quote'),
      ('mail.delivery', 'event', 'A message went from one address to another.', NULL, NULL),
      -- cloud
      ('cloud.account', 'entity', 'A cloud account, subscription or project.', 'trim', NULL),
      ('cloud.bucket', 'entity', 'An object-storage bucket.', 'lowercase', 'container'),
      ('cloud.instance', 'entity', 'A virtual machine or container instance.', 'trim', 'server'),
      ('cloud.role', 'entity', 'A role or service principal.', 'trim', 'id-card'),
      ('cloud.access_key', 'entity', 'An access key, by its id -- never the secret.', 'trim', 'key-square'),
      ('cloud.region', 'entity', 'A region or zone.', 'lowercase', 'earth'),
      ('cloud.api_call', 'event', 'A control-plane call was made.', NULL, NULL),
      -- host
      ('host.id', 'entity', 'A machine identifier: a GUID, serial or asset tag.', 'lowercase', 'badge'),
      ('host.os', 'entity', 'An operating system and version.', 'trim', 'layers'),
      ('host.service', 'entity', 'A service or daemon.', 'trim', 'cog'),
      ('host.task', 'entity', 'A scheduled task or cron job.', 'trim', 'timer'),
      ('host.driver', 'entity', 'A driver or kernel module.', 'trim', 'puzzle'),
      ('host.volume', 'entity', 'A volume or mount point.', 'trim', 'hard-drive'),
      ('host.share', 'entity', 'A network share.', 'lowercase', 'folder-tree'),
      ('host.pipe', 'entity', 'A named pipe.', 'trim', 'cable'),
      ('host.boot', 'event', 'A host started.', NULL, NULL),
      ('host.shutdown', 'event', 'A host shut down.', NULL, NULL),
      ('host.service.install', 'event', 'A service was installed.', NULL, NULL),
      ('host.task.create', 'event', 'A scheduled task was created.', NULL, NULL),
      ('host.address', 'observation', 'A host had an address.', NULL, NULL),
      -- device
      ('device.id', 'entity', 'A device identifier: a UDID or serial.', 'trim', NULL),
      ('device.model', 'entity', 'A device model.', 'trim', 'tablet-smartphone'),
      ('device.imei', 'entity', 'An IMEI.', 'trim', NULL),
      ('device.usb', 'entity', 'A USB device, by vendor, product and serial.', 'trim', 'usb'),
      ('device.connect', 'event', 'A device was connected.', NULL, NULL),
      -- app
      ('app.bundle', 'entity', 'An application identifier: a bundle id or package name.', 'lowercase', NULL),
      ('app.version', 'entity', 'An application version.', 'trim', 'tag'),
      ('app.entitlement', 'entity', 'A permission or entitlement an application holds.', 'trim', 'shield-check'),
      ('app.install', 'event', 'An application was installed.', NULL, NULL),
      -- account, auth
      ('account.sid', 'entity', 'A Windows security identifier.', 'uppercase', 'id-card'),
      ('account.group', 'entity', 'A group of accounts.', 'trim', 'users'),
      ('account.credential', 'entity', 'A credential that was seen, by what identifies it -- never the secret.', 'trim', 'key-round'),
      ('account.member_of', 'observation', 'An account belongs to a group.', NULL, NULL),
      ('auth.session', 'entity', 'A logon session, by its id.', 'trim', NULL),
      ('auth.logon', 'event', 'An account logged on.', NULL, NULL),
      ('auth.logoff', 'event', 'An account logged off.', NULL, NULL),
      ('auth.failure', 'event', 'A logon was refused.', NULL, NULL),
      ('auth.privilege', 'event', 'Privileges were gained or used.', NULL, NULL),
      -- person, org, geo, doc
      ('person.name', 'entity', 'A person, by name.', 'trim', NULL),
      ('person.handle', 'entity', 'A name someone goes by online.', 'lowercase', 'at-sign'),
      ('org.name', 'entity', 'An organisation, by name.', 'trim', NULL),
      ('org.sector', 'entity', 'An industry or sector.', 'lowercase', 'factory'),
      ('geo.country', 'entity', 'A country.', 'trim', 'earth'),
      ('geo.place', 'entity', 'A city, site or address.', 'trim', NULL),
      ('geo.coordinates', 'entity', 'A latitude and longitude.', 'trim', 'locate'),
      ('doc.report', 'entity', 'A report or article, by its title or URL.', 'trim', NULL),
      ('doc.reference', 'entity', 'A citation or ticket number.', 'trim', 'bookmark'),
      -- file
      ('file.dir', 'entity', 'A directory on some host.', 'trim', 'folder'),
      ('file.magic', 'entity', 'A file format, as identified.', 'lowercase', 'file-question-mark'),
      ('file.signer', 'entity', 'Who signed a file: the certificate''s subject.', 'trim', 'signature'),
      ('file.hash.sha512', 'entity', 'A SHA-512 digest of a file, in hex.', 'lowercase', NULL),
      ('file.hash.ssdeep', 'entity', 'An ssdeep fuzzy hash.', 'trim', NULL),
      ('file.hash.tlsh', 'entity', 'A TLSH locality hash.', 'uppercase', NULL),
      ('file.hash.imphash', 'entity', 'A PE import hash.', 'lowercase', NULL),
      ('file.create', 'event', 'A file was created.', NULL, NULL),
      ('file.write', 'event', 'A file was written.', NULL, NULL),
      ('file.read', 'event', 'A file was read.', NULL, NULL),
      ('file.rename', 'event', 'A file was renamed or moved.', NULL, NULL),
      ('file.delete', 'event', 'A file was deleted.', NULL, NULL),
      ('file.execute', 'event', 'A file was run.', NULL, NULL),
      ('file.present', 'observation', 'A file was at a path on a host.', NULL, NULL),
      ('file.digest', 'observation', 'A file has a digest.', NULL, NULL),
      ('file.contains', 'observation', 'An archive or image holds a file.', NULL, NULL),
      -- process, registry, mem
      ('process.cmdline', 'entity', 'A command line as run.', 'trim', 'terminal'),
      ('process.module', 'entity', 'A module loaded into a process.', 'trim', 'package'),
      ('process.start', 'event', 'A process started.', NULL, NULL),
      ('process.exit', 'event', 'A process ended.', NULL, NULL),
      ('process.inject', 'event', 'One process wrote into another.', NULL, NULL),
      ('process.access', 'event', 'One process opened another.', NULL, NULL),
      ('process.module_load', 'event', 'A process loaded a module.', NULL, NULL),
      ('registry.value', 'entity', 'A registry value: its key and name.', 'trim', 'braces'),
      ('registry.set', 'event', 'A registry value was written.', NULL, NULL),
      ('registry.delete', 'event', 'A registry key or value was removed.', NULL, NULL),
      ('mem.image', 'entity', 'A memory image.', 'trim', 'hard-drive-download'),
      ('mem.region', 'entity', 'A region of a process''s memory, by its base address.', 'lowercase', NULL),
      ('mem.handle', 'entity', 'A handle a process holds.', 'trim', 'grip'),
      ('mem.injection', 'observation', 'Memory that does not belong to what it sits in.', NULL, NULL),
      -- evidence
      ('evidence.item', 'entity', 'A piece of evidence, by its exhibit number.', 'trim', 'package'),
      ('evidence.image', 'entity', 'A disk image.', 'trim', 'disc'),
      ('evidence.log', 'entity', 'A log source.', 'trim', 'scroll-text'),
      ('evidence.custodian', 'entity', 'Who held a piece of evidence.', 'trim', 'user-round'),
      ('evidence.acquire', 'event', 'Evidence was acquired.', NULL, NULL),
      ('evidence.transfer', 'event', 'Evidence changed hands.', NULL, NULL),
      -- threat, attack
      ('threat.actor', 'entity', 'A threat actor, by the name it is tracked under.', 'trim', 'ghost'),
      ('threat.intrusion_set', 'entity', 'A set of activity tracked as one.', 'trim', NULL),
      ('threat.campaign', 'entity', 'A campaign.', 'trim', 'flag'),
      ('threat.tool', 'entity', 'A tool an actor uses that is not malware.', 'trim', 'wrench'),
      ('threat.infrastructure', 'entity', 'Infrastructure an actor operates.', 'trim', 'server'),
      ('threat.attribution', 'observation', 'Activity is attributed to an actor.', NULL, NULL),
      ('threat.uses', 'observation', 'An actor or campaign uses a tool, malware or technique.', NULL, NULL),
      ('threat.targets', 'observation', 'An actor or campaign targets a victim, sector or place.', NULL, NULL),
      ('threat.indicates', 'observation', 'An indicator points at a threat.', NULL, NULL),
      ('attack.tactic', 'entity', 'An ATT&CK tactic.', 'trim', 'milestone'),
      ('attack.technique', 'entity', 'An ATT&CK technique, by its id (T1059.001).', 'uppercase', NULL),
      ('attack.mitigation', 'entity', 'An ATT&CK mitigation.', 'trim', 'shield-check'),
      ('attack.observed', 'observation', 'A technique was seen in use.', NULL, NULL),
      -- detect, vuln
      ('detect.rule', 'entity', 'A detection rule, by name.', 'trim', 'siren'),
      ('detect.yara', 'entity', 'A YARA rule.', 'trim', 'scan-search'),
      ('detect.sigma', 'entity', 'A Sigma rule.', 'trim', 'sigma'),
      ('detect.snort', 'entity', 'A Snort or Suricata rule, by its sid.', 'trim', NULL),
      ('detect.alert', 'event', 'A rule fired.', NULL, NULL),
      ('detect.match', 'observation', 'A rule matches something.', NULL, NULL),
      ('vuln.cve', 'entity', 'A CVE id.', 'uppercase', NULL),
      ('vuln.cwe', 'entity', 'A CWE id.', 'uppercase', 'shield-question-mark'),
      ('vuln.advisory', 'entity', 'A vendor or CERT advisory.', 'trim', 'megaphone'),
      ('vuln.exploit', 'entity', 'An exploit, by name or reference.', 'trim', 'bomb'),
      ('vuln.affects', 'observation', 'A vulnerability affects a product or host.', NULL, NULL),
      ('vuln.exploited', 'event', 'A vulnerability was exploited.', NULL, NULL),
      -- malware
      ('malware.family', 'entity', 'A malware family.', 'trim', NULL),
      ('malware.sample', 'entity', 'A specimen, by the name it is analysed under.', 'trim', 'biohazard'),
      ('malware.config', 'entity', 'A configuration recovered from a sample.', 'trim', 'file-cog'),
      ('malware.c2', 'entity', 'A command-and-control endpoint as configured.', 'trim', 'radio-tower'),
      ('malware.mutex', 'entity', 'A mutex a sample creates.', 'trim', 'lock'),
      ('malware.packer', 'entity', 'A packer or crypter.', 'trim', 'package'),
      ('malware.capability', 'entity', 'Something a sample can do.', 'trim', 'blocks'),
      ('malware.variant_of', 'observation', 'A sample belongs to a family, or one family derives from another.', NULL, NULL),
      ('malware.drops', 'observation', 'A sample writes or unpacks another file.', NULL, NULL),
      ('malware.contacts', 'observation', 'A sample talks to an endpoint.', NULL, NULL),
      ('malware.detonation', 'event', 'A sample was run in a sandbox.', NULL, NULL),
      -- code
      ('code.binary', 'entity', 'A binary or module under analysis.', 'trim', NULL),
      ('code.function', 'entity', 'A function, by name or address.', 'trim', 'square-function'),
      ('code.symbol', 'entity', 'A symbol or global.', 'trim', 'variable'),
      ('code.string', 'entity', 'A string found in a binary.', 'trim', 'quote'),
      ('code.address', 'entity', 'An address in a binary.', 'lowercase', 'locate'),
      ('code.section', 'entity', 'A section or segment.', 'trim', 'layers'),
      ('code.import', 'entity', 'An imported function or library.', 'trim', 'package-plus'),
      ('code.export', 'entity', 'An exported function.', 'trim', 'package-minus'),
      ('code.type', 'entity', 'A structure, class or enum.', 'trim', 'braces'),
      ('code.constant', 'entity', 'A constant worth a name: a magic number, a seed, a table.', 'trim', 'equal'),
      ('code.syscall', 'entity', 'A system call or API a binary relies on.', 'trim', 'command'),
      ('code.calls', 'observation', 'One function calls another.', NULL, NULL),
      ('code.references', 'observation', 'Code refers to a string, symbol or address.', NULL, NULL),
      ('code.defines', 'observation', 'A binary holds a function, symbol or string.', NULL, NULL),
      ('code.implements', 'observation', 'A function implements an algorithm or capability.', NULL, NULL),
      -- crypto, software, misc
      ('crypto.algorithm', 'entity', 'A cipher, hash or protocol.', 'lowercase', NULL),
      ('crypto.key', 'entity', 'A key, by its fingerprint or id -- never the secret.', 'trim', 'key-square'),
      ('crypto.cert', 'entity', 'A certificate, by its SHA-256 fingerprint.', 'lowercase', 'file-badge'),
      ('crypto.wallet', 'entity', 'A cryptocurrency address.', 'trim', 'wallet'),
      ('software.name', 'entity', 'A product or program.', 'trim', NULL),
      ('software.version', 'entity', 'A product and its version.', 'trim', 'tag'),
      ('software.package', 'entity', 'A package as its ecosystem names it.', 'trim', 'boxes'),
      ('software.installed', 'observation', 'Software is installed on a host.', NULL, NULL),
      ('misc.thing', 'entity', 'A thing that has no better type.', 'trim', NULL),
      ('misc.label', 'entity', 'A name or label to hang things on.', 'trim', 'tag'),
      ('misc.event', 'event', 'Something happened that has no better type.', NULL, NULL),
      ('misc.link', 'observation', 'Two things are related; the type says no more.', NULL, NULL);

    INSERT OR IGNORE INTO type_alias(alias, name) VALUES
      ('cidr', 'net.cidr'), ('asn', 'net.asn'), ('mac', 'net.mac'), ('ja3', 'net.tls.ja3'),
      ('sha512', 'file.hash.sha512'), ('ssdeep', 'file.hash.ssdeep'), ('tlsh', 'file.hash.tlsh'),
      ('imphash', 'file.hash.imphash'), ('sid', 'account.sid'),
      ('cve', 'vuln.cve'), ('cwe', 'vuln.cwe'), ('technique', 'attack.technique'),
      ('actor', 'threat.actor'), ('campaign', 'threat.campaign'),
      ('family', 'malware.family'), ('mutex', 'malware.mutex'),
      ('function', 'code.function'), ('string', 'code.string'), ('yara', 'detect.yara');
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
