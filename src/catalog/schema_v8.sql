CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 8),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE control_operations (
    operation_id TEXT PRIMARY KEY CHECK(length(operation_id) BETWEEN 16 AND 64 AND operation_id NOT GLOB '*[^A-Za-z0-9_-]*' AND substr(operation_id, 1, 1) GLOB '[A-Za-z0-9]'),
    kind TEXT NOT NULL CHECK(kind IN ('ensure_member', 'set_source_paused', 'retire_member', 'retire_source', 'set_group_priorities', 'sync_source_models', 'set_group_pin', 'clear_volcano_error', 'update_volcano_policy', 'discover_source_models', 'create_token', 'set_token_enabled', 'delete_token')),
    target_type TEXT NOT NULL CHECK(target_type IN ('source', 'group', 'membership', 'token')),
    target_id INTEGER NOT NULL CHECK(target_id > 0),
    payload_digest TEXT NOT NULL CHECK(length(payload_digest) = 64 AND payload_digest NOT GLOB '*[^0-9a-f]*'),
    target_revision INTEGER NOT NULL CHECK(target_revision BETWEEN 1 AND 9007199254740991),
    expected_targets INTEGER NOT NULL CHECK(expected_targets BETWEEN 0 AND 10000),
    phase TEXT NOT NULL CHECK(phase IN ('queued', 'applying', 'waiting', 'partial', 'unknown', 'succeeded', 'failed', 'superseded')),
    error_code TEXT CHECK(error_code IN ('invalid_input', 'not_found', 'revision_conflict', 'idempotency_conflict', 'queue_full', 'not_ready', 'binding_drift', 'ambiguous_create', 'auth_failed', 'remote_rejected', 'invalid_response', 'rate_limited', 'transport_unknown', 'storage')),
    created_at INTEGER NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
    updated_at INTEGER NOT NULL CHECK(updated_at BETWEEN 0 AND 9007199254740991),
    CHECK((kind IN ('ensure_member', 'retire_member') AND target_type = 'membership')
       OR (kind IN ('set_source_paused', 'retire_source', 'sync_source_models', 'clear_volcano_error', 'update_volcano_policy', 'discover_source_models') AND target_type = 'source')
       OR (kind IN ('set_group_priorities', 'set_group_pin') AND target_type = 'group')
       OR (kind IN ('create_token', 'set_token_enabled', 'delete_token') AND target_type = 'token')),
    CHECK(phase != 'succeeded' OR error_code IS NULL),
    CHECK(phase != 'failed' OR error_code IS NOT NULL),
    CHECK(target_type != 'membership' OR expected_targets = 1),
    CHECK(kind NOT IN ('set_group_pin', 'clear_volcano_error', 'update_volcano_policy') OR (expected_targets = 0 AND phase = 'succeeded')),
    CHECK(target_type != 'token' OR (expected_targets = 1 AND phase NOT IN ('partial', 'superseded')))
) STRICT;
CREATE TABLE managed_tokens (
    token_id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_user_id INTEGER NOT NULL CHECK(owner_user_id > 0),
    owner_scope TEXT NOT NULL CHECK(owner_scope IN ('issuer', 'legacy_admin')),
    native_token_id INTEGER UNIQUE CHECK(native_token_id > 0),
    group_id INTEGER REFERENCES source_groups(group_id) ON DELETE RESTRICT,
    explicit_group TEXT NOT NULL CHECK(length(CAST(explicit_group AS BLOB)) <= 256),
    display_name TEXT NOT NULL CHECK(length(CAST(display_name AS BLOB)) BETWEEN 1 AND 256),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    evidence_revision INTEGER NOT NULL CHECK(evidence_revision BETWEEN 0 AND 9007199254740991),
    create_operation_id TEXT UNIQUE REFERENCES control_operations(operation_id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    create_group_revision INTEGER CHECK(create_group_revision BETWEEN 1 AND 9007199254740991),
    expires_at INTEGER CHECK(expires_at BETWEEN 0 AND 9007199254740991),
    created_at INTEGER NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
    deleted_at INTEGER CHECK(deleted_at BETWEEN created_at AND 9007199254740991),
    CHECK((create_operation_id IS NULL AND native_token_id IS NOT NULL AND create_group_revision IS NULL AND expires_at IS NULL)
       OR (create_operation_id IS NOT NULL AND owner_scope = 'issuer' AND group_id IS NOT NULL AND create_group_revision IS NOT NULL AND explicit_group != '' AND explicit_group != 'auto')),
    CHECK(deleted_at IS NULL OR native_token_id IS NOT NULL)
) STRICT;
CREATE TABLE token_operation_targets (
    operation_id TEXT PRIMARY KEY REFERENCES control_operations(operation_id) ON DELETE RESTRICT,
    token_id INTEGER NOT NULL REFERENCES managed_tokens(token_id) ON DELETE RESTRICT,
    token_revision INTEGER NOT NULL CHECK(token_revision BETWEEN 1 AND 9007199254740991),
    desired_enabled INTEGER CHECK(desired_enabled IN (0, 1)),
    confirmed_at INTEGER CHECK(confirmed_at BETWEEN 0 AND 9007199254740991),
    receipt_json TEXT CHECK(json_valid(receipt_json) AND json_type(receipt_json) = 'object'),
    CHECK((confirmed_at IS NULL) = (receipt_json IS NULL))
) STRICT;
CREATE UNIQUE INDEX token_targets_identity ON token_operation_targets(token_id, token_revision);
CREATE INDEX token_pending ON control_operations(updated_at, operation_id) WHERE target_type = 'token' AND phase IN ('queued', 'waiting', 'unknown');
CREATE TABLE token_execution_attempts (
    token_id INTEGER NOT NULL REFERENCES managed_tokens(token_id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK(sequence BETWEEN 1 AND 9007199254740991),
    operation_id TEXT NOT NULL REFERENCES token_operation_targets(operation_id) ON DELETE RESTRICT,
    token_revision INTEGER NOT NULL CHECK(token_revision BETWEEN 1 AND 9007199254740991),
    action TEXT NOT NULL CHECK(action IN ('create', 'enable', 'disable', 'delete')),
    state TEXT NOT NULL CHECK(state IN ('prepared', 'responded', 'not_sent', 'uncertain')),
    error_code TEXT CHECK(error_code IN ('invalid_input', 'not_found', 'revision_conflict', 'idempotency_conflict', 'queue_full', 'not_ready', 'binding_drift', 'ambiguous_create', 'auth_failed', 'remote_rejected', 'invalid_response', 'rate_limited', 'transport_unknown', 'storage')),
    started_at INTEGER NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740991),
    finished_at INTEGER CHECK(finished_at BETWEEN started_at AND 9007199254740991),
    PRIMARY KEY(token_id, sequence),
    CHECK((state = 'prepared') = (finished_at IS NULL)),
    CHECK(state != 'uncertain' OR error_code IS 'transport_unknown'),
    CHECK(state != 'not_sent' OR error_code IS NOT NULL),
    CHECK(state != 'prepared' OR error_code IS NULL)
) STRICT;
CREATE UNIQUE INDEX token_one_prepared ON token_execution_attempts(token_id) WHERE state = 'prepared';
CREATE INDEX token_attempt_operation ON token_execution_attempts(operation_id, sequence);
