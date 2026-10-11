# Landing examples

Existing Grida home artwork reused for the FX landing page at the repository owner's request. These are curated presentation assets, not complete workflow run packages. Material and campaign illustrations are not evidence of an actual FX execution.

## Source and reuse basis

- Source repository: [gridaco/grida](https://github.com/gridaco/grida).
- Source commit: `165496f60e30afdeecfd750df7e01f1ba5750cbf`.
- Source root: `editor/public/www/2026-10/`.
- Reuse basis: explicit owner instruction to use the existing Grida home examples in the local FX landing page. Source-code licensing is not used to infer rights to these media assets.
- No new image generation, provider run, or media recompression was performed for this copy. Image and video files retain the source bytes.
- The world manifest changes only public asset URLs to `/fx/landing/examples/world/` and omits the unused preview entry.
- Landing artwork is separate from the downloadable run/snapshot contract.
- The chair sketch in `src/components/landing-showcase.tsx` is adapted from the
  same commit's `editor/app/(www)/(home)/_home/product-design-preview.tsx` under
  the source repository's Apache-2.0 code license.

## Assets

World images are 1280 × 720. Hills, trees, ground, and foreground include alpha; source and sky are opaque. `materials.webp` is 1536 × 1024. Campaign headphones are 900 × 900 with alpha, social is 900 × 900, poster is 800 × 1200, and motion poster is 720 × 1280. The 7.96-second motion video is H.264 at 720 × 1280 without an audio track.

The existing curated WebP and MP4 encodings are retained. The video is about 1.13 MB; clients should request it only when the campaign example is used.

Paths below are relative to this directory and the source root respectively.

| Local file | Source file | Bytes |
| --- | --- | ---: |
| `world/source.webp` | `wind-garden-repaint/source.webp` | 168,956 |
| `world/sky.webp` | `wind-garden-repaint/sky.webp` | 47,666 |
| `world/hills.webp` | `wind-garden-repaint/hills.webp` | 133,892 |
| `world/trees.webp` | `wind-garden-repaint/trees.webp` | 184,524 |
| `world/ground.webp` | `wind-garden-repaint/ground.webp` | 206,720 |
| `world/foreground.webp` | `wind-garden-repaint/foreground.webp` | 77,082 |
| `materials.webp` | `fx-product-design/material-studies.webp` | 62,220 |
| `campaign/headphones.webp` | `fx-campaign/headphones.webp` | 81,146 |
| `campaign/poster-v1.webp` | `fx-campaign/poster-v1.webp` | 54,498 |
| `campaign/social-v1.webp` | `fx-campaign/social-v1.webp` | 60,918 |
| `campaign/motion-v2-poster.webp` | `fx-campaign/motion-v2-poster.webp` | 50,712 |
| `campaign/motion-v2.mp4` | `fx-campaign/motion-v2.mp4` | 1,126,263 |
| `world/manifest.json` | `wind-garden-repaint/manifest.json` | 1,838 |

## SHA-256

| Local file | Source SHA-256 | Copy SHA-256 |
| --- | --- | --- |
| `world/source.webp` | `620bd8212c6e6b14b9b0e99f6b7f598d7c38bbc810dc45e27286b4fac2215dc3` | `620bd8212c6e6b14b9b0e99f6b7f598d7c38bbc810dc45e27286b4fac2215dc3` |
| `world/sky.webp` | `197c007c25e7392558611d5d854ace3cccd70e5185d64e03d7b0334a0ede4661` | `197c007c25e7392558611d5d854ace3cccd70e5185d64e03d7b0334a0ede4661` |
| `world/hills.webp` | `58de0e180daf86d80b7af50dd5cbf6aef033d8a0b1b08a6dcf5cda2233c53d02` | `58de0e180daf86d80b7af50dd5cbf6aef033d8a0b1b08a6dcf5cda2233c53d02` |
| `world/trees.webp` | `6dffc951faf55b8e9c3517a98ba0d35a9a653b8f24273387f8f9a8a8a3325e3e` | `6dffc951faf55b8e9c3517a98ba0d35a9a653b8f24273387f8f9a8a8a3325e3e` |
| `world/ground.webp` | `dfec7a5570baf458d3ca1d87439095891e8b4484c3668ec6f9151f0690a81418` | `dfec7a5570baf458d3ca1d87439095891e8b4484c3668ec6f9151f0690a81418` |
| `world/foreground.webp` | `0c1fcb295cd1a391a3f675e018210a25ec0942e115fa874d8b155f467f6d9b72` | `0c1fcb295cd1a391a3f675e018210a25ec0942e115fa874d8b155f467f6d9b72` |
| `materials.webp` | `da6b20450b28ef35211b8e815aae88f3454761b261931710f78a33226e51bb10` | `da6b20450b28ef35211b8e815aae88f3454761b261931710f78a33226e51bb10` |
| `campaign/headphones.webp` | `ce126ce866065c40724ef19cb8c3490dd97d2f26e281cf0783668ca18fdb7388` | `ce126ce866065c40724ef19cb8c3490dd97d2f26e281cf0783668ca18fdb7388` |
| `campaign/poster-v1.webp` | `c4f7fe3bbe6136493474fe3925e52b6d0f781c985700243aed31c8d48606d7f0` | `c4f7fe3bbe6136493474fe3925e52b6d0f781c985700243aed31c8d48606d7f0` |
| `campaign/social-v1.webp` | `53c442723e09fb96e480bfdc12cbbd6aa98a6ee6222e80b77e2170e72b23ea41` | `53c442723e09fb96e480bfdc12cbbd6aa98a6ee6222e80b77e2170e72b23ea41` |
| `campaign/motion-v2-poster.webp` | `e60427e603c5488e256a90c83398bf3752f6058108b32398fa864e2019fa6b4f` | `e60427e603c5488e256a90c83398bf3752f6058108b32398fa864e2019fa6b4f` |
| `campaign/motion-v2.mp4` | `a3cc624b0739a5bc1d3604dcf37883cf5ffc17cd95512d8f90267f6db1f3819b` | `a3cc624b0739a5bc1d3604dcf37883cf5ffc17cd95512d8f90267f6db1f3819b` |
| `world/manifest.json` | `19b7e9b14e942332e5872fdaa492c63b954b020ac91c6f4689b064685355facf` | `7fd202fcb8c23815c53f5e09e60128d85b8465ac7291bb3cd4ab0683061f5afb` |

Total asset bytes (including the rewritten manifest, excluding this README): **2,256,435**.
