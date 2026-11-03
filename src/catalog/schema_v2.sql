CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 2),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE source_control (
    source_id INTEGER PRIMARY KEY REFERENCES sources(source_id) ON DELETE RESTRICT,
    retire_requested INTEGER NOT NULL CHECK(retire_requested IN (0, 1))
) STRICT;
CREATE TABLE member_control (
    membership_id INTEGER PRIMARY KEY REFERENCES memberships(membership_id) ON DELETE RESTRICT,
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    status_directive TEXT NOT NULL CHECK(status_directive IN ('preserve', 'enabled', 'disabled')),
    retire_requested INTEGER NOT NULL CHECK(retire_requested IN (0, 1)),
    create_state TEXT NOT NULL CHECK(create_state IN ('never', 'may_have_been_sent', 'resolved')),
    create_name TEXT NOT NULL UNIQUE,
    dirty INTEGER NOT NULL CHECK(dirty IN (0, 1)),
    next_attempt_at INTEGER NOT NULL CHECK(next_attempt_at BETWEEN 0 AND 9007199254740991),
    last_error_code TEXT CHECK(last_error_code IN ('invalid_input', 'not_found', 'revision_conflict', 'idempotency_conflict', 'queue_full', 'not_ready', 'binding_drift', 'ambiguous_create', 'auth_failed', 'remote_rejected', 'invalid_response', 'rate_limited', 'transport_unknown', 'storage'))
) STRICT;
CREATE INDEX control_dirty_due ON member_control(dirty, next_attempt_at, membership_id);
CREATE TABLE control_operations (
    operation_id TEXT PRIMARY KEY CHECK(length(operation_id) BETWEEN 16 AND 64 AND operation_id NOT GLOB '*[^A-Za-z0-9_-]*' AND substr(operation_id, 1, 1) GLOB '[A-Za-z0-9]'),
    kind TEXT NOT NULL CHECK(kind IN ('ensure_member', 'set_source_paused', 'retire_member', 'retire_source', 'set_group_priorities', 'sync_source_models')),
    target_type TEXT NOT NULL CHECK(target_type IN ('source', 'group', 'membership')),
    target_id INTEGER NOT NULL CHECK(target_id > 0),
    payload_digest TEXT NOT NULL CHECK(length(payload_digest) = 64 AND payload_digest NOT GLOB '*[^0-9a-f]*'),
    target_revision INTEGER NOT NULL CHECK(target_revision BETWEEN 1 AND 9007199254740991),
    expected_targets INTEGER NOT NULL CHECK(expected_targets BETWEEN 0 AND 10000),
    phase TEXT NOT NULL CHECK(phase IN ('queued', 'applying', 'waiting', 'partial', 'unknown', 'succeeded', 'failed', 'superseded')),
    error_code TEXT CHECK(error_code IN ('invalid_input', 'not_found', 'revision_conflict', 'idempotency_conflict', 'queue_full', 'not_ready', 'binding_drift', 'ambiguous_create', 'auth_failed', 'remote_rejected', 'invalid_response', 'rate_limited', 'transport_unknown', 'storage')),
    created_at INTEGER NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
    updated_at INTEGER NOT NULL CHECK(updated_at BETWEEN 0 AND 9007199254740991),
    CHECK((kind IN ('ensure_member', 'retire_member') AND target_type = 'membership')
       OR (kind IN ('set_source_paused', 'retire_source', 'sync_source_models') AND target_type = 'source')
       OR (kind = 'set_group_priorities' AND target_type = 'group')),
    CHECK(phase != 'succeeded' OR error_code IS NULL),
    CHECK(phase != 'failed' OR error_code IS NOT NULL),
    CHECK(target_type != 'membership' OR expected_targets = 1)
) STRICT;
CREATE TABLE operation_targets (
    operation_id TEXT NOT NULL REFERENCES control_operations(operation_id) ON DELETE RESTRICT,
    membership_id INTEGER NOT NULL REFERENCES memberships(membership_id) ON DELETE RESTRICT,
    target_member_revision INTEGER NOT NULL CHECK(target_member_revision BETWEEN 1 AND 9007199254740991),
    confirmed_at INTEGER CHECK(confirmed_at BETWEEN 0 AND 9007199254740991),
    receipt_json TEXT CHECK(receipt_json IS NULL OR (json_valid(receipt_json) AND json_type(receipt_json) = 'object')),
    PRIMARY KEY(operation_id, membership_id),
    CHECK((confirmed_at IS NULL) = (receipt_json IS NULL))
) STRICT;
CREATE INDEX operation_targets_member ON operation_targets(membership_id, target_member_revision);
