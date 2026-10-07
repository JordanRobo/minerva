-- Roadmap 3.6 (D4): one canonical row per task relationship.
--
-- Existing rows that violate the new constraints are cleaned up first so the
-- migration is safe on databases with test data: self-relations are deleted,
-- and duplicate or reversed pairs keep only the oldest row (earliest
-- created_at, then lowest id). Pre-existing `blocked_by` rows are left as
-- they are: they violate none of the new constraints, and application writes
-- no longer produce them.

-- A task cannot relate to itself.
DELETE FROM task_relations WHERE source_task_id = target_task_id;

-- Duplicate or reversed `blocks` rows: keep the oldest per unordered pair.
DELETE FROM task_relations a
USING task_relations b
WHERE a.relation_type = 'blocks'
  AND b.relation_type = 'blocks'
  AND LEAST(a.source_task_id, a.target_task_id) = LEAST(b.source_task_id, b.target_task_id)
  AND GREATEST(a.source_task_id, a.target_task_id) = GREATEST(b.source_task_id, b.target_task_id)
  AND (a.created_at, a.id) > (b.created_at, b.id);

-- Same for `relates_to`: the pair matters, not which endpoint was submitted first.
DELETE FROM task_relations a
USING task_relations b
WHERE a.relation_type = 'relates_to'
  AND b.relation_type = 'relates_to'
  AND LEAST(a.source_task_id, a.target_task_id) = LEAST(b.source_task_id, b.target_task_id)
  AND GREATEST(a.source_task_id, a.target_task_id) = GREATEST(b.source_task_id, b.target_task_id)
  AND (a.created_at, a.id) > (b.created_at, b.id);

ALTER TABLE task_relations
    ADD CONSTRAINT task_relations_no_self_relation CHECK (source_task_id <> target_task_id);

-- One row per unordered pair, per type: duplicates and direct reverse loops
-- are impossible at the database level with no race window.
CREATE UNIQUE INDEX task_relations_unique_blocks_pair
    ON task_relations (LEAST(source_task_id, target_task_id), GREATEST(source_task_id, target_task_id))
    WHERE relation_type = 'blocks';

CREATE UNIQUE INDEX task_relations_unique_relates_to_pair
    ON task_relations (LEAST(source_task_id, target_task_id), GREATEST(source_task_id, target_task_id))
    WHERE relation_type = 'relates_to';
