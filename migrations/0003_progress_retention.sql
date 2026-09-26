-- Supports ordered, bounded retention batches without scanning all profiles.
CREATE INDEX progress_updated_at_idx ON progress (updated_at);
