CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 5),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE ledger_state (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    initial_requested_from INTEGER CHECK(initial_requested_from BETWEEN 0 AND 9007199254740991)
) STRICT;
CREATE TABLE ledger_checkpoints (
    lane TEXT PRIMARY KEY NOT NULL CHECK(lane IN ('recent','history')),
    generation INTEGER NOT NULL CHECK(generation BETWEEN 0 AND 9007199254740991),
    window_from INTEGER CHECK(window_from BETWEEN 0 AND 9007199254740991),
    window_to INTEGER CHECK(window_to BETWEEN 1 AND 9007199254740991),
    next_page INTEGER NOT NULL CHECK(next_page BETWEEN 1 AND 9007199254740991),
    phase TEXT NOT NULL CHECK(phase IN ('idle','scanning','scanned','stalled')),
    last_observed_at INTEGER CHECK(last_observed_at BETWEEN 0 AND 9007199254740991),
    fault_code TEXT CHECK(fault_code IN ('transport','invalid_page','invalid_fact','conflicting_identity','binding_changed','history_unavailable','resource_limit','clock')),
    CHECK((phase='idle' AND generation=0 AND window_from IS NULL AND window_to IS NULL AND next_page=1 AND last_observed_at IS NULL AND fault_code IS NULL)
        OR (phase!='idle' AND generation>0 AND window_from IS NOT NULL AND window_to IS NOT NULL AND window_from<window_to
            AND ((phase='scanning' AND fault_code IS NULL AND (last_observed_at IS NULL OR last_observed_at>=window_to))
                OR (phase='scanned' AND fault_code IS NULL AND last_observed_at IS NOT NULL AND last_observed_at>=window_to)
                OR (phase='stalled' AND fault_code IS NOT NULL AND last_observed_at IS NOT NULL))))
) STRICT;
CREATE TABLE ledger_facts (
    native_log_id INTEGER PRIMARY KEY CHECK(native_log_id > 0),
    request_id TEXT CHECK(length(CAST(request_id AS BLOB)) BETWEEN 1 AND 64),
    kind TEXT NOT NULL CHECK(kind IN ('consume','error')),
    occurred_at INTEGER NOT NULL CHECK(occurred_at BETWEEN 0 AND 9007199254740991),
    channel_id INTEGER CHECK(channel_id > 0),
    source_id INTEGER REFERENCES sources(source_id) ON DELETE RESTRICT CHECK(source_id > 0),
    group_id INTEGER REFERENCES source_groups(group_id) ON DELETE RESTRICT CHECK(group_id > 0),
    native_group TEXT CHECK(length(CAST(native_group AS BLOB)) BETWEEN 1 AND 256),
    token_id INTEGER CHECK(token_id > 0),
    model TEXT CHECK(length(CAST(model AS BLOB)) BETWEEN 1 AND 256),
    input INTEGER NOT NULL CHECK(input BETWEEN 0 AND 9007199254740991),
    output INTEGER NOT NULL CHECK(output BETWEEN 0 AND 9007199254740991),
    is_stream INTEGER NOT NULL CHECK(is_stream IN (0,1)),
    metadata_json TEXT NOT NULL CHECK(length(CAST(metadata_json AS BLOB)) BETWEEN 1 AND 16384),
    attribution_issue TEXT NOT NULL CHECK(attribution_issue IN ('none','unmapped_channel','group_unresolved')),
    CHECK(input+output <= 9007199254740991),
    CHECK(kind!='error' OR (input=0 AND output=0)),
    CHECK((attribution_issue='none' AND channel_id IS NOT NULL AND source_id IS NOT NULL AND group_id IS NOT NULL AND native_group IS NOT NULL)
        OR (attribution_issue='unmapped_channel' AND source_id IS NULL AND group_id IS NULL)
        OR (attribution_issue='group_unresolved' AND channel_id IS NOT NULL AND source_id IS NOT NULL AND group_id IS NULL))
) STRICT;
CREATE TABLE ledger_minute_usage (
    minute INTEGER NOT NULL CHECK(minute BETWEEN 0 AND 9007199254740991 AND minute%60=0),
    channel_id INTEGER CHECK(channel_id > 0),
    source_id INTEGER REFERENCES sources(source_id) ON DELETE RESTRICT CHECK(source_id > 0),
    group_id INTEGER REFERENCES source_groups(group_id) ON DELETE RESTRICT CHECK(group_id > 0),
    native_group TEXT CHECK(length(CAST(native_group AS BLOB)) BETWEEN 1 AND 256),
    token_id INTEGER CHECK(token_id > 0),
    model TEXT CHECK(length(CAST(model AS BLOB)) BETWEEN 1 AND 256),
    input INTEGER NOT NULL CHECK(input BETWEEN 0 AND 9007199254740991),
    output INTEGER NOT NULL CHECK(output BETWEEN 0 AND 9007199254740991),
    consumption_rows INTEGER NOT NULL CHECK(consumption_rows BETWEEN 1 AND 9007199254740991),
    CHECK(input+output <= 9007199254740991),
    CHECK((group_id IS NULL OR (source_id IS NOT NULL AND native_group IS NOT NULL)) AND (source_id IS NULL OR channel_id IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX ledger_cell_key ON ledger_minute_usage(minute,coalesce(channel_id,0),coalesce(source_id,0),coalesce(group_id,0),coalesce(native_group,''),coalesce(token_id,0),coalesce(model,''));
CREATE INDEX ledger_fact_time ON ledger_facts(occurred_at,native_log_id);
CREATE INDEX ledger_fact_token_time ON ledger_facts(token_id,occurred_at,native_log_id);
CREATE INDEX ledger_fact_channel ON ledger_facts(channel_id,native_log_id);
CREATE INDEX ledger_fact_projection ON ledger_facts(occurred_at-occurred_at%60,coalesce(channel_id,0),coalesce(source_id,0),coalesce(group_id,0),coalesce(native_group,''),coalesce(token_id,0),coalesce(model,''),native_log_id) WHERE kind='consume';
CREATE INDEX ledger_cell_time ON ledger_minute_usage(minute);
CREATE INDEX ledger_cell_token_time ON ledger_minute_usage(token_id,minute);
