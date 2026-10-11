# Export conformance evidence

`minimal.json` is an independently authored v1 terminal recording, not generated
by the exporter. `minimal.sha256` pins its exact UTF-8 bytes. Its synthetic source
event vector is `[ {"event":"run_started"}, {"event":"run_finished"} ]`; remove
whitespace through RFC 8785 before hashing. Readers must preserve withheld
markers and reject an unsupported kind, non-null observation cursor, traversal
artifact path, inconsistent inventory or absent required layout.
