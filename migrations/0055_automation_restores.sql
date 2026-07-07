-- Timed hold: an automation can put everything it touched back after a while
-- ("hall lights on at 30% — put things back after 10 minutes").
ALTER TABLE automations ADD COLUMN restore_secs INTEGER;

-- One pending restore per rule: the pre-fire snapshot of every device the
-- rule's actions touch (models::automation::RestoreEntry JSON), applied and
-- deleted when restore_at passes. A re-fire during the hold extends
-- restore_at but keeps the ORIGINAL snapshot — otherwise the "restored" state
-- would be the rule's own triggered state. Persisted so a hold survives a
-- restart. Deleting the rule cancels its hold.
CREATE TABLE automation_restores (
    automation_id TEXT PRIMARY KEY REFERENCES automations(id) ON DELETE CASCADE,
    restore_at TEXT NOT NULL,
    snapshot_json TEXT NOT NULL
);
