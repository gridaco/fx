# Riko

Riko is an adopted canonical character for FX character-content examples. Her
default reference is `full_body.webp`: an adult woman with honey-blonde hair,
pink ends, a high side ponytail, raspberry cropped sweater, denim shorts and
white high-top sneakers. Her personality is confident, playful and welcoming.
These personality cues are optional creative direction. Source-game lore and
relationships are not required; FX examples may invent their own roles and stories.

The [adoption manifest](riko.json) pins eight byte-identical source files: the
profile, three illustrations, a rigged SD model, its preview, generation record
and independent review. FX owns the local copy under
`www/.showcases/canonical-characters/riko/`. Historical source paths in preserved
records describe lineage only; consumers resolve the corresponding local files.
No source-repository checkout is required to read these assets.

The default illustration, loose-sweater illustration and SD representation are
distinct references. Choose the representation explicitly for each workflow;
do not silently substitute the SD model for the illustrated character. The
model has fixed facial features, fingers and hair, and its existing review
covers only the recorded diagnostic poses, not arbitrary dance motion.

Generated media stays outside Git. Before delivering a portable public example,
publish the selected bytes to FX-owned delivery storage and bind them through
the consuming example's checksum-verified input manifest. This local adoption
does not claim CDN publication or a new motion-quality verdict.
