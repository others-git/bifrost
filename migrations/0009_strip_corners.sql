-- LED strips can corner: replace the single end tile (x2, y2) with `points`,
-- a JSON array of [x, y] vertices the strip passes through after its start
-- tile. NULL means a point light.
ALTER TABLE plan_lights ADD COLUMN points TEXT;

UPDATE plan_lights
SET points = json_array(json_array(x2, y2))
WHERE x2 IS NOT NULL AND y2 IS NOT NULL;

ALTER TABLE plan_lights DROP COLUMN x2;
ALTER TABLE plan_lights DROP COLUMN y2;
