CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 4),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE volcano_runtime (
    source_id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'volcano' CHECK(kind = 'volcano'),
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision BETWEEN 0 AND 9007199254740991),
    period_id INTEGER CHECK(period_id BETWEEN 1 AND 9007199254740991),
    period_start INTEGER CHECK(period_start BETWEEN 0 AND 9007199254740991),
    next_reset_at INTEGER CHECK(next_reset_at BETWEEN 0 AND 9007199254740991),
    used_input INTEGER NOT NULL DEFAULT 0 CHECK(used_input BETWEEN 0 AND 9007199254740991),
    used_output INTEGER NOT NULL DEFAULT 0 CHECK(used_output BETWEEN 0 AND 9007199254740991),
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK(consecutive_failures BETWEEN 0 AND 9007199254740991),
    error_exhausted INTEGER NOT NULL DEFAULT 0 CHECK(error_exhausted IN (0, 1)),
    limit_exhausted INTEGER NOT NULL DEFAULT 0 CHECK(limit_exhausted IN (0, 1)),
    error_epoch INTEGER NOT NULL DEFAULT 0 CHECK(error_epoch BETWEEN 0 AND 9007199254740991),
    error_cutoff_at INTEGER CHECK(error_cutoff_at BETWEEN 0 AND 9007199254740991),
    evidence_cursor INTEGER CHECK(evidence_cursor BETWEEN 0 AND 9007199254740991),
    coverage_end INTEGER CHECK(coverage_end BETWEEN 0 AND 9007199254740991),
    FOREIGN KEY(source_id, kind) REFERENCES sources(source_id, kind) ON DELETE RESTRICT,
    CHECK(used_input + used_output <= 9007199254740991),
    CHECK((revision = 0 AND period_id IS NULL AND period_start IS NULL AND next_reset_at IS NULL
        AND used_input = 0 AND used_output = 0 AND consecutive_failures = 0
        AND error_exhausted = 0 AND limit_exhausted = 0 AND error_epoch = 0
        AND error_cutoff_at IS NULL AND evidence_cursor IS NULL AND coverage_end IS NULL)
      OR (revision > 0 AND period_id IS NOT NULL AND period_start IS NOT NULL AND next_reset_at IS NOT NULL
        AND period_start < next_reset_at AND error_epoch > 0
        AND (error_cutoff_at IS NULL OR (error_cutoff_at >= period_start AND error_cutoff_at < next_reset_at))
        AND (coverage_end IS NULL OR (coverage_end >= period_start AND coverage_end <= next_reset_at))
        AND (evidence_cursor IS NULL OR coverage_end IS NOT NULL)))
) STRICT;
CREATE TABLE group_policy_state (
    group_id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('glm', 'volcano')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision BETWEEN 1 AND 9007199254740991),
    active_member_id INTEGER REFERENCES memberships(membership_id) ON DELETE RESTRICT,
    pinned_member_id INTEGER REFERENCES memberships(membership_id) ON DELETE RESTRICT,
    regime TEXT CHECK(regime IN ('normal', 'degraded')),
    cursor_order INTEGER CHECK(cursor_order BETWEEN 0 AND 9007199254740991),
    cursor_member_id INTEGER REFERENCES memberships(membership_id) ON DELETE RESTRICT,
    FOREIGN KEY(group_id, kind) REFERENCES source_groups(group_id, kind) ON DELETE RESTRICT,
    CHECK((kind = 'glm' AND regime IS NOT NULL AND cursor_order IS NULL AND cursor_member_id IS NULL)
       OR (kind = 'volcano' AND regime IS NULL AND pinned_member_id IS NULL
        AND (cursor_order IS NULL) = (cursor_member_id IS NULL)
        AND (active_member_id IS NULL OR (cursor_member_id IS NOT NULL AND active_member_id = cursor_member_id))))
) STRICT;
