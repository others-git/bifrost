-- Automations gain condition-able ACTION STEPS: a rule's "then" is now a list
-- of steps, each `{conditions, actions}`, so a single rule can branch (e.g.
-- "dim the lights always, but only open Hulu after 18:00" — two steps, the
-- second time-window-gated; two steps with opposite conditions are an if/else).
--
-- The step list reuses the existing `actions_json` column (its content shape
-- changes, not the schema). Wrap every existing flat action array
-- `[a, b, …]` into a single unconditional step `[{"conditions": [], "actions":
-- [a, b, …]}]`, preserving behaviour for every current rule.
UPDATE automations
SET actions_json = json_array(
        json_object('conditions', json('[]'), 'actions', json(actions_json))
    )
WHERE json_valid(actions_json);
