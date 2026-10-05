<!-- A prompt template, for the `question:` form of a review. It uses the same ${{ }} expressions as
     the workflow file; `vars` from the step are in scope, plus `inputs`. The rendered prompt is
     recorded with the result. The workflow now uses `criteria:` instead (see
     guide/06-annotations-and-judges.md), which answers each point separately and pins failures;
     this file is kept to show templates. -->
You are reviewing one concept image for a storyworld. You did not draw it.

Entity: ${{ entity.name }} (${{ entity.mode }})
What it must show: ${{ direction.must_show }}
What it must not show: ${{ direction.must_avoid }}

Answer `accept` only if all of these hold:
1. The image shows exactly this entity, recognisably, and nothing that contradicts the description.
2. There is no text, logo, watermark or frame.
3. It follows the set's visual grammar: ${{ direction.grammar_summary }}

Otherwise answer `reject` and give one reason per failed point.
