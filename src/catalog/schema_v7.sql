CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 7),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE control_operations (
    operation_id TEXT PRIMARY KEY CHECK(length(operation_id) BETWEEN 16 AND 64 AND operation_id NOT GLOB '*[^A-Za-z0-9_-]*' AND substr(operation_id, 1, 1) GLOB '[A-Za-z0-9]'),
    kind TEXT NOT NULL CHECK(kind IN ('ensure_member', 'set_source_paused', 'retire_member', 'retire_source', 'set_group_priorities', 'sync_source_models', 'set_group_pin', 'clear_volcano_error', 'update_volcano_policy', 'discover_source_models')),
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
       OR (kind IN ('set_source_paused', 'retire_source', 'sync_source_models', 'clear_volcano_error', 'update_volcano_policy', 'discover_source_models') AND target_type = 'source')
       OR (kind IN ('set_group_priorities', 'set_group_pin') AND target_type = 'group')),
    CHECK(phase != 'succeeded' OR error_code IS NULL),
    CHECK(phase != 'failed' OR error_code IS NOT NULL),
    CHECK(target_type != 'membership' OR expected_targets = 1),
    CHECK(kind NOT IN ('set_group_pin', 'clear_volcano_error', 'update_volcano_policy') OR (expected_targets = 0 AND phase = 'succeeded'))
) STRICT;
