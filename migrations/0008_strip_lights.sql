-- LED strips: a placement may span a run of tiles. When (x2, y2) is set the
-- light renders as a segment from (x, y) to (x2, y2) (same mount offset at
-- both ends); NULL means a point light as before.
ALTER TABLE plan_lights ADD COLUMN x2 INTEGER;
ALTER TABLE plan_lights ADD COLUMN y2 INTEGER;
