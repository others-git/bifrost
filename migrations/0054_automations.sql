-- Automations generalize sensor rules: the trigger becomes a tagged JSON
-- input (models::automation::AutomationTrigger) — a sensor event today, other
-- input kinds later — so new trigger kinds need no schema change. `sensor_id`
-- stays as a denormalized index of the sensor-input trigger (NULL for future
-- non-sensor kinds): the engine looks up an event's automations in one
-- indexed query, and a sensor's automations are removed with it.
CREATE TABLE automations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    trigger_json TEXT NOT NULL,
    conditions_json TEXT NOT NULL DEFAULT '[]',
    actions_json TEXT NOT NULL DEFAULT '[]',
    cooldown_secs INTEGER NOT NULL DEFAULT 0,
    sensor_id TEXT REFERENCES sensor_devices(id) ON DELETE CASCADE,
    last_fired_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_automations_sensor ON automations(sensor_id);

-- Carry over any rules created as bare sensor rules, wrapping each stored
-- event into the tagged sensor-input trigger shape.
INSERT INTO automations (id, name, enabled, trigger_json, conditions_json, actions_json, cooldown_secs, sensor_id, last_fired_at, created_at)
SELECT id, name, enabled,
       json_object('kind', 'sensor', 'sensor_id', sensor_id, 'event', json(trigger_json)),
       conditions_json, actions_json, cooldown_secs, sensor_id, last_fired_at, created_at
FROM sensor_rules;

DROP TABLE sensor_rules;
