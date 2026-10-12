Plan one focused software-launch clip from the supplied source storyboard, source metadata,
and exact-copy brief. The storyboard is a sequential sampling of a supplied recording,
not a set of separate assets. Source pixels will be played continuously at an even speed.

The renderer has three consecutive parts: animated opening typography, the supplied
recording with smooth camera framing, and animated closing typography. Copy comes verbatim
from the brief; never rewrite or invent UI behavior. Return only camera and timing decisions.
Prefer a deliberate 17-20 second total. Use 1.6-2.8 seconds opening, 12-14 seconds footage,
and 2-3.2 seconds closing. Keep the complete source visible at first and last body beats.

Focus beats are relative seconds in the footage part, sorted, beginning at 0 and ending
at footage_seconds. x/y are source-image fractions. zoom 1 means the whole recording card;
zoom above 1 enlarges exact recording pixels about the focus. Use 3-6 beats, limit zoom to
2.1, and allow at least 1 second between beats. A brief may give focus_hints with source
seconds and source fractions: respect what matters at those times and map their timing
into the body duration. For a 12-second source, do not accelerate playback: use at least 12 seconds of footage.
Point/zoom moves inside one crop take the final 0.75 seconds before each focus beat.
Different crop regions switch with clean editorial cuts 0.75 seconds before the target
beat, then hold. They never sweep through half-visible inactive panes.
Allow at least 2 seconds at the final canvas-focused beat before the final wide beat.
Keep canvas-reveal zoom at or below 1.5 so the complete key illustration stays visible.
Favor sustained readable closeups over constant movement. Return
an editorial note identifying what the camera is following. Do not claim to have listened
to audio: this renderer makes a silent clip for later assembly.

When the brief supplies named regions and region names in focus_hints, use those exact
normalized [left, top, right, bottom] source crop bounds on the corresponding focus beats.
Use zoom 1 for region crops: they already frame complete content. Use [0,0,1,1] at the
first and last beats. Cropping isolates active content; never infer or repaint missing UI.
Without named regions, use [0,0,1,1] and the point/zoom controls described above.
