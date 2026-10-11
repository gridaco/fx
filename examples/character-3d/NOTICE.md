# Source attribution

The character workflow, prompts, profiles and private `_character_tools` helpers were
adapted from Stage Gen's character-3D pipeline, copyright (c) 2026, softmarshmallow.
Their BSD 3-Clause terms are preserved in `LICENSE.stage-gen`.

Changes relocate the source into one standalone FX example, rename private imports and
make the Blender bootstrap copy its local worker package. The generation, review,
assembly, rigging and numeric validation algorithms are preserved. These private helpers
are example implementation details, not additions to the FX SDK.

Generated showcase artwork and run records are not included or licensed by this source
notice. Offline fixture media is synthesized by `make_fixture.py`.
