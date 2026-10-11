# Movie sprite processing

`finish_video(raw_video: bytes, config: dict) -> dict[str, bytes]` performs local
sprite finishing. `FinishSettings` is the strict configuration model;
`validate_finish_config` admits it without media or providers.
`inspect_source_video` probes a source and decodes all frames with bounded work.
The caller owns artifact persistence, provenance and provider execution. The
[movie-sprite workflow](../../workflow.yaml) provides those
through the public FX SDK.

## Settings

| Field | Default and meaning |
| --- | --- |
| `source_mode` | `chroma`; `rgba` skips keying; `finished` preserves already closed RGBA frames |
| `playback_seconds` | `12`; `null` retains source rate; no frames are duplicated or interpolated by retiming |
| `chroma` | Green-excess extraction and bounded edge despill; see `ChromaSettings` |
| `fixed_regions` | Empty list of `{ "polygon_xy": [[x, y], ...] }` polygons pinned to frame zero |
| `moving_guards` | Same polygon form; subtracts moving areas from fixed fields |
| `outside_feather_pixels` | `12` native pixels outside fixed fields |
| `source_sha256`, `coordinate_size` | Required for any geometry; exact compressed-source hash and native `[width, height]` |
| `loop_closure` | `flow`; `none` requires already identical endpoints |
| `seam_frames` | `12`; symmetric windows of this many source frames at each end |
| `flow_proxy_width` | `720`; flow analysis width, with bounded vectors applied at native scale |
| `max_flow_pixels` | `36` in flow proxy coordinates |
| `local_repairs` | Empty list of optional opaque attachment repairs |
| `preview_max_size` | `[720, 1280]`; contain resize for opaque MP4 preview |
| `export_frames` | `false`; enables a numbered lossless PNG ZIP |

A local repair contains `polygon_xy`, `roi_xyxy: [x0, y0, x1, y1]`,
`falloff_pixels` (60), `pin_feather_pixels` (3), and
`max_displacement_pixels` (36). The ROI supplies context for local alignment;
the compact field settles it into an exact first-frame pin. This is for small
opaque attachments, not automatic whole-body pose correction. All spatial
coordinates refer to the exact selected video's native canvas.

Chroma defaults use the green-excess workflow:
`key_rgb: [11, 242, 18]`, `key_excess: 224`, `foreground_excess: 8`,
`minimum_alpha: 24`, `despill_radius_pixels: 3`. The source must use a green backing;
green foreground material or ambiguous edges may require different extraction or
caller-supplied RGBA. Review compositing over representative backgrounds.

## Processing and bounds

The component decodes frames to temporary raw files, keys green sources,
subtracts moving guards from fixed fields, applies premultiplied-alpha feathering,
and closes the loop with bounded optical flow. Optional attachment alignment pins
small defects without spreading a freeze across the body. Middle frames outside
authored corrections retain their original keyed motion.

Output `video` is native, lossless straight RGBA in FFV1 Matroska. Every encoded
frame is independently decoded and compared before return; the canonical PNG and
last frame must exactly equal the first processed frame. `manifest`, `report`,
`preview`, `contact_sheet` and optional `frames_zip` accompany it. The caller should
publish them as one admitted node result. The next facial pipeline consumes
`canonical`; this component does not repaint facial states or implement playback.

Supported bounds are 360 frames, at most 60 seconds and 60 source fps, even native
dimensions up to 2160×3840 or 3840×2160, 1 GiB compressed input and 2 GiB compressed
outputs. FFmpeg/ffprobe are required. Raw file access reads or writes one frame at
a time without mapping the whole sequence into memory. Native 4K runs need tens of
gigabytes of free scratch space for input/output RGBA, preview RGB and compressed
artifacts; the preflight includes all of these. Encoder file limits and a combined
output budget refuse oversized artifacts. Compressed artifacts are
returned as bytes for the existing harness. This bounds processing without adding
a new streaming SDK or an engine-specific format.

Configuration/source mismatch, excessive allocation, variable frame timing,
decode failure, nonmatching finished endpoints and lossless verification failure
are refused. These are structural checks. They do not certify artistic motion,
edge quality or suitability of caller-selected regions.
