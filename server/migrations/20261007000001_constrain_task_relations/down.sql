-- Rows removed by the up migration (self-relations, duplicate or reversed
-- pairs) are not restored on rollback.
DROP INDEX IF EXISTS task_relations_unique_relates_to_pair;
DROP INDEX IF EXISTS task_relations_unique_blocks_pair;
ALTER TABLE task_relations DROP CONSTRAINT IF EXISTS task_relations_no_self_relation;
