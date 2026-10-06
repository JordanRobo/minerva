-- Roadmap 2.8 (step 4): fixed-window rate-limit counters, used only while
-- Postgres holds them (Redis keeps the same counters as keys with native
-- TTLs and needs no table).
--
-- One row per (counter key, window start). The key embeds the policy name and
-- the SHA-256 hash of the subject — never the raw IP or email. `hit` is a
-- single atomic upsert (`INSERT … ON CONFLICT DO UPDATE … RETURNING count`),
-- so concurrent hits increment without interleaving; an hourly maintenance job
-- deletes rows whose window has fully passed (see interface maintenance).
CREATE TABLE rate_limit_counters (
    key TEXT NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (key, window_start)
);
