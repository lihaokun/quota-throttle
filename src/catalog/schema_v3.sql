CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 3),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE member_execution (
    membership_id INTEGER PRIMARY KEY REFERENCES memberships(membership_id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK(sequence BETWEEN 0 AND 9007199254740991),
    enable_epoch INTEGER NOT NULL CHECK(enable_epoch BETWEEN 0 AND 9007199254740991),
    enable_requested_revision INTEGER NOT NULL CHECK(enable_requested_revision BETWEEN 0 AND 9007199254740991),
    enable_consumed INTEGER NOT NULL CHECK(enable_consumed IN (0, 1)),
    CHECK((enable_epoch = 0) = (enable_requested_revision = 0)),
    CHECK(enable_consumed = 0 OR enable_epoch > 0)
) STRICT;
CREATE TABLE execution_attempts (
    membership_id INTEGER NOT NULL REFERENCES member_execution(membership_id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK(sequence BETWEEN 1 AND 9007199254740991),
    member_revision INTEGER NOT NULL CHECK(member_revision BETWEEN 1 AND 9007199254740991),
    source_revision INTEGER NOT NULL CHECK(source_revision BETWEEN 1 AND 9007199254740991),
    group_revision INTEGER NOT NULL CHECK(group_revision BETWEEN 1 AND 9007199254740991),
    enable_epoch INTEGER NOT NULL CHECK(enable_epoch BETWEEN 0 AND 9007199254740991),
    action TEXT NOT NULL CHECK(action IN ('create', 'fields', 'enable', 'disable')),
    effect_json TEXT NOT NULL CHECK(json_valid(effect_json) AND json_type(effect_json) = 'object'),
    state TEXT NOT NULL CHECK(state IN ('prepared', 'responded', 'not_sent', 'uncertain')),
    error_code TEXT CHECK(error_code IN ('invalid_input', 'not_found', 'revision_conflict', 'idempotency_conflict', 'queue_full', 'not_ready', 'binding_drift', 'ambiguous_create', 'auth_failed', 'remote_rejected', 'invalid_response', 'rate_limited', 'transport_unknown', 'storage')),
    started_at INTEGER NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740991),
    finished_at INTEGER CHECK(finished_at BETWEEN 0 AND 9007199254740991),
    PRIMARY KEY(membership_id, sequence),
    CHECK((state = 'prepared') = (finished_at IS NULL))
) STRICT;
CREATE INDEX execution_unresolved ON execution_attempts(membership_id, state, sequence);
CREATE UNIQUE INDEX execution_one_prepared ON execution_attempts(membership_id) WHERE state = 'prepared';
