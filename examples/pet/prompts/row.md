A sprite strip for a small animated desktop pet: ${{ vars.frames }} frames of one animation, drawn
side by side in a single horizontal row and read from left to right.

The pet, ${{ inputs.name }}, is the character in input image 1: ${{ inputs.look }}
Draw that same character in every frame, with its shape, proportions, colours, face and markings
exactly as in input image 1. Only its pose there is not kept.

Style: ${{ vars.style }}

The animation: ${{ vars.action }}

- Exactly ${{ vars.frames }} full-body poses of the same pet, evenly spaced, with clear empty space
  between neighbours. No pose touches another pose or the edge of the picture.
- The pet is identical in every frame and to input image 1: the same shape, proportions, face,
  colours, markings and size. Only its pose changes, a little from one frame to the next, so the
  frames loop smoothly.
- Its feet stay on one shared ground line, unless the animation leaves the ground.
- It must read clearly when each frame is shrunk to about 190 by 200 pixels: a compact
  silhouette, bold shapes, no fine detail.
- Nothing but the pet: no ground, shadow, scenery, text, numbers, frame borders, speed lines, or
  effects floating apart from its body.
