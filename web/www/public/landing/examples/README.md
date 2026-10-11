# Landing example

One curated landscape example reused from Grida home at the repository owner's
request. These are presentation assets, not a complete FX run package or recorded
execution evidence.

## Source and reuse basis

- Source repository: [gridaco/grida](https://github.com/gridaco/grida).
- Source commit: `165496f60e30afdeecfd750df7e01f1ba5750cbf`.
- Source root: `editor/public/www/2026-10/wind-garden-repaint/`.
- Reuse basis: explicit owner instruction to use the Grida home example in FX.
  Source-code licensing is not used to infer media redistribution rights.
- The manifest rebases public asset URLs to `/fx/landing/examples/world/` and
  omits the unused preview entry.
- Material and campaign examples and their media have been removed.

## Encoding

All six images are WebP at 1280 × 720. Hills, trees, ground and foreground
include alpha; source and sky are opaque. The copied WebPs were recompressed
with Sharp 0.35.5 using `webp({ quality: 82, effort: 6, alphaQuality: 100 })`.
Dimensions and decoded alpha pixels are unchanged; RGB compression is lossy.
An independent visual comparison found no obvious degradation at ordinary
viewing size. No new generation or provider call was performed.

| File | Bytes |
| --- | ---: |
| `world/source.webp` | 125,902 |
| `world/sky.webp` | 31,046 |
| `world/hills.webp` | 113,364 |
| `world/trees.webp` | 160,442 |
| `world/ground.webp` | 137,998 |
| `world/foreground.webp` | 68,940 |
| `world/manifest.json` | 1,838 |

Total including the manifest, excluding this README: **639,530 bytes**.

## SHA-256

Source hashes identify the original WebPs from the pinned Grida commit. Local
hashes identify the optimized files shipped here.

| File | Source SHA-256 | Local SHA-256 |
| --- | --- | --- |
| `world/source.webp` | `620bd8212c6e6b14b9b0e99f6b7f598d7c38bbc810dc45e27286b4fac2215dc3` | `4d1122289d9db91985def851cf4a07e9df7f52cea3c7b9bb39a4c8e6419543d2` |
| `world/sky.webp` | `197c007c25e7392558611d5d854ace3cccd70e5185d64e03d7b0334a0ede4661` | `d1c5b0d0f586d233a62f63bb16ca5ed04536a1b8dd34b56d3497560ea2fb8702` |
| `world/hills.webp` | `58de0e180daf86d80b7af50dd5cbf6aef033d8a0b1b08a6dcf5cda2233c53d02` | `5f7c5225dbb14db3003e15bdf37580f649fdc70a312f4726d4e16d160f70c679` |
| `world/trees.webp` | `6dffc951faf55b8e9c3517a98ba0d35a9a653b8f24273387f8f9a8a8a3325e3e` | `c59d054d44890890a78216657eea09b84d6ef7aa18623fd13cfc46b5cfd9b940` |
| `world/ground.webp` | `dfec7a5570baf458d3ca1d87439095891e8b4484c3668ec6f9151f0690a81418` | `14c2f074f10d925bd88c0a7226e45d6e2997c8379972aabfd129d0de96d97b52` |
| `world/foreground.webp` | `0c1fcb295cd1a391a3f675e018210a25ec0942e115fa874d8b155f467f6d9b72` | `d9d1d3c8467ece012b816277b6c65e4d75cc7109ae9d809de277477b83a1ee49` |
| `world/manifest.json` | `19b7e9b14e942332e5872fdaa492c63b954b020ac91c6f4689b064685355facf` | `7fd202fcb8c23815c53f5e09e60128d85b8465ac7291bb3cd4ab0683061f5afb` |
