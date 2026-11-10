Use case: identity-preserve.
Asset type: one full-face repaint, registered to a canonical portrait.
State id: ${{ vars.state_id }}

Input image 1 is the finished MASTER face. Return ONE image with that same COMPLETE
face and surrounding hair, neck and clothing in their original positions. Keep the
same square canvas, framing, pose, scale, proportions, colors, linework, shading,
lighting, hair strands and character identity. Do not crop, zoom, rotate, center,
beautify, redesign or reconstruct the face from detached pieces. No atlas or sheet.

Change only this requested feature, painting it in context on the original face:
${{ vars.instruction }}

Keep every other facial feature and its position unchanged. In particular preserve
eye spacing, cheek and jaw shape, nose position, hair silhouette and the exact
relationship between all features. Keep the existing anime drawing style. Repair
skin under only the feature being changed. The surrounding complete face is the
constraint, not a style reference. No labels, masks, diagrams, text or watermarks.

Return the complete repainted portrait on the same neutral backdrop. A local step
will align this full image and transfer only the explicitly masked region; no
separate eye, iris, lip, head or hair cutouts should appear in this image.
