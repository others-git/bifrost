-- A receiver binding names which receiver inputs mean "the receiver is serving
-- this source", independently of the one input selected on power-on
-- (`receiver_source`). A receiver can reach one source over several inputs
-- (HDMI and ARC, say). While the receiver sits on one of these inputs, surfaces
-- fold it into the source's control; on any other input it shows on its own.
-- JSON array of input names; NULL = none named (falls back to receiver_source).
ALTER TABLE media_devices ADD COLUMN receiver_match_inputs TEXT;
