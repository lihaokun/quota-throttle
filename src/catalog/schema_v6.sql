CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 6),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE volcano_observation_queue (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK(sequence BETWEEN 1 AND 9007199254740991),
    native_log_id INTEGER NOT NULL UNIQUE REFERENCES ledger_facts(native_log_id) ON DELETE RESTRICT CHECK(native_log_id > 0),
    source_id INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'volcano' CHECK(kind='volcano'),
    FOREIGN KEY(source_id,kind) REFERENCES sources(source_id,kind) ON DELETE RESTRICT
) STRICT;
CREATE INDEX volcano_queue_source ON volcano_observation_queue(source_id,sequence);
CREATE TABLE volcano_request_observations (
    source_id INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'volcano' CHECK(kind='volcano'),
    request_id TEXT NOT NULL CHECK(length(CAST(request_id AS BLOB)) BETWEEN 1 AND 64 AND request_id NOT GLOB '*[^A-Za-z0-9_-]*'),
    first_event_at INTEGER NOT NULL CHECK(first_event_at BETWEEN 0 AND 9007199254740991),
    phase TEXT NOT NULL CHECK(phase IN ('failed','token_flow')),
    PRIMARY KEY(source_id,request_id),
    FOREIGN KEY(source_id,kind) REFERENCES sources(source_id,kind) ON DELETE RESTRICT
) STRICT;
CREATE TABLE volcano_period_history (
    source_id INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'volcano' CHECK(kind='volcano'),
    period_id INTEGER NOT NULL CHECK(period_id BETWEEN 1 AND 9007199254740991),
    old_start INTEGER NOT NULL CHECK(old_start BETWEEN 0 AND 9007199254740991),
    old_next INTEGER NOT NULL CHECK(old_next BETWEEN 0 AND 9007199254740991),
    new_start INTEGER NOT NULL CHECK(new_start BETWEEN 0 AND 9007199254740991),
    new_next INTEGER NOT NULL CHECK(new_next BETWEEN 0 AND 9007199254740991),
    reset_minute INTEGER NOT NULL CHECK(reset_minute BETWEEN 0 AND 1439),
    utc_offset_minutes INTEGER NOT NULL CHECK(utc_offset_minutes BETWEEN -720 AND 840),
    CHECK(old_start < old_next AND old_next <= new_start AND new_start < new_next),
    PRIMARY KEY(source_id,period_id),
    FOREIGN KEY(source_id,kind) REFERENCES sources(source_id,kind) ON DELETE RESTRICT
) STRICT;
CREATE INDEX ledger_cell_source_time ON ledger_minute_usage(source_id,minute);
CREATE INDEX ledger_fact_source_usage ON ledger_facts(source_id,occurred_at,native_log_id) WHERE kind='consume';
CREATE INDEX ledger_fact_source_request ON ledger_facts(source_id,request_id,occurred_at);
