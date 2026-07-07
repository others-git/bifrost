-- Sensor automation rules: "when <this sensor does X> [only if <conditions>]
-- then <actions>". Trigger/conditions/actions are typed JSON (serde enums in
-- models::automation); actions replay through the shared service layer.
CREATE TABLE sensor_rules (
    id TEXT PRIMARY KEY,
    sensor_id TEXT NOT NULL REFERENCES sensor_devices(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    trigger_json TEXT NOT NULL,
    conditions_json TEXT NOT NULL DEFAULT '[]',
    actions_json TEXT NOT NULL DEFAULT '[]',
    cooldown_secs INTEGER NOT NULL DEFAULT 0,
    last_fired_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_sensor_rules_sensor ON sensor_rules(sensor_id);
