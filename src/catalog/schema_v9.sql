CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 9),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE balance_policy (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    auto_refill INTEGER NOT NULL CHECK(auto_refill IN (0, 1)),
    warn_below INTEGER NOT NULL CHECK(warn_below >= 1),
    refill_to INTEGER NOT NULL CHECK(refill_to <= 4000000000000000),
    CHECK(warn_below < refill_to)
) STRICT;
