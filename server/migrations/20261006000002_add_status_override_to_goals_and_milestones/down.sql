-- Roadmap 3.2 (D12): drop the manual status override columns again.
ALTER TABLE goals DROP COLUMN status_override;
ALTER TABLE milestones DROP COLUMN status_override;
