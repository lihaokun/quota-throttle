CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 1)
);
CREATE TABLE source_secrets (
    secret_id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(secret_id > 0),
    raw_key TEXT NOT NULL CHECK(length(raw_key) > 0)
);
CREATE TABLE sources (
    source_id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(source_id > 0),
    kind TEXT NOT NULL CHECK(kind IN ('glm', 'volcano')),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    note TEXT NOT NULL CHECK(length(note) <= 2048),
    secret_id INTEGER NOT NULL UNIQUE REFERENCES source_secrets(secret_id) ON DELETE RESTRICT,
    credential_fingerprint TEXT NOT NULL UNIQUE,
    settings_json TEXT NOT NULL,
    config_revision INTEGER NOT NULL CHECK(config_revision BETWEEN 1 AND 9007199254740991),
    manual_paused INTEGER NOT NULL CHECK(manual_paused IN (0, 1)),
    retired_at INTEGER CHECK(retired_at BETWEEN 0 AND 9007199254740991),
    UNIQUE(source_id, kind)
);
CREATE TABLE source_groups (
    group_id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(group_id > 0),
    newapi_group TEXT NOT NULL UNIQUE CHECK(length(newapi_group) BETWEEN 1 AND 64 AND newapi_group != 'auto'),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    kind TEXT NOT NULL CHECK(kind IN ('glm', 'volcano')),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    ready INTEGER NOT NULL CHECK(ready IN (0, 1)),
    UNIQUE(group_id, kind)
);
CREATE TABLE memberships (
    membership_id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(membership_id > 0),
    source_id INTEGER NOT NULL,
    group_id INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('glm', 'volcano')),
    channel_id INTEGER UNIQUE CHECK(channel_id > 0),
    position INTEGER NOT NULL CHECK(position BETWEEN 0 AND 9007199254740991),
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('planned', 'imported', 'created_disabled', 'ready', 'retired')),
    desired_priority INTEGER NOT NULL CHECK(desired_priority BETWEEN -9007199254740991 AND 9007199254740991),
    observed_status INTEGER CHECK(observed_status IN (1, 2, 3)),
    observed_priority INTEGER CHECK(observed_priority BETWEEN -9007199254740991 AND 9007199254740991),
    applied_revision INTEGER CHECK(applied_revision BETWEEN 1 AND 9007199254740991),
    retired_at INTEGER CHECK(retired_at BETWEEN 0 AND 9007199254740991),
    FOREIGN KEY(source_id, kind) REFERENCES sources(source_id, kind) ON DELETE RESTRICT,
    FOREIGN KEY(group_id, kind) REFERENCES source_groups(group_id, kind) ON DELETE RESTRICT,
    CHECK((lifecycle = 'planned' AND channel_id IS NULL)
       OR (lifecycle IN ('imported', 'created_disabled', 'ready') AND channel_id IS NOT NULL)
       OR lifecycle = 'retired'),
    CHECK((lifecycle = 'retired') = (retired_at IS NOT NULL))
);
CREATE UNIQUE INDEX active_source_group ON memberships(source_id, group_id) WHERE lifecycle != 'retired';
CREATE UNIQUE INDEX active_group_position ON memberships(group_id, position) WHERE lifecycle != 'retired';
CREATE TABLE legacy_import (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    digest TEXT NOT NULL,
    receipt_json TEXT NOT NULL
);
