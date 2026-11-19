# Changelog

User-facing changes to Grida FX. Earlier versions list selected highlights.

## Unreleased

- For provider failures, `grida fx run` points to the exact run and shows a safe explanation when available. `inspect` and both SDKs show saved failed attempts, including failures recovered by retry. Codex failures include ephemeral thread and turn IDs when available. [#148](https://github.com/gridaco/fx/pull/148)
- Improved browser tab metadata and discovery of FX Viewer documents for host apps. [#145](https://github.com/gridaco/fx/pull/145)

## [0.10.0](https://github.com/gridaco/fx/tree/v0.10.0)

- Start from editable workflow templates with `grida fx init` and `add`, and use provider credentials saved in Grida. [#143](https://github.com/gridaco/fx/pull/143), [#140](https://github.com/gridaco/fx/pull/140)

## [0.9.0](https://github.com/gridaco/fx/tree/v0.9.0)

- Choose models and providers separately, discover compatible image models, and explore recorded workflow values in the viewer. [#132](https://github.com/gridaco/fx/pull/132), [#133](https://github.com/gridaco/fx/pull/133), [#139](https://github.com/gridaco/fx/pull/139)

## [0.8.1](https://github.com/gridaco/fx/tree/v0.8.1)

- Added an embedded viewer browser client, single-image Tripo mesh generation, and structured provider diagnostics. [#127](https://github.com/gridaco/fx/pull/127), [#124](https://github.com/gridaco/fx/pull/124)

## [0.8.0](https://github.com/gridaco/fx/tree/v0.8.0)

- Added configurable OpenAI-compatible requests and billing, plus author-selected video posters. [#109](https://github.com/gridaco/fx/pull/109), [#120](https://github.com/gridaco/fx/pull/120)

## [0.7.1](https://github.com/gridaco/fx/tree/v0.7.1)

- Reissued 0.7.0 to unblock npm installation on Linux x64. [#108](https://github.com/gridaco/fx/pull/108)

## [0.7.0](https://github.com/gridaco/fx/tree/v0.7.0)

- Added project-declared providers and adapters, more flexible model routes, and per-call caps. [#106](https://github.com/gridaco/fx/pull/106)

## [0.6.1](https://github.com/gridaco/fx/tree/v0.6.1)

- Added Grok Build image generation and video previews in the viewer. [#76](https://github.com/gridaco/fx/pull/76), [#78](https://github.com/gridaco/fx/pull/78)

## [0.6.0](https://github.com/gridaco/fx/tree/v0.6.0)

- Added TypeScript nodes, workflow builders, and stand-ins. [#71](https://github.com/gridaco/fx/pull/71)

## [0.5.1](https://github.com/gridaco/fx/tree/v0.5.1)

- Added project starters, examples, and skills through `grida fx init`. [#59](https://github.com/gridaco/fx/pull/59)

## [0.5.0](https://github.com/gridaco/fx/tree/v0.5.0)

- Added Codex reference images, planning-time request checks, and visible waiting states. [#58](https://github.com/gridaco/fx/pull/58)

## [0.4.0](https://github.com/gridaco/fx/tree/v0.4.0)

- Added Grida Gateway as a provider. [#49](https://github.com/gridaco/fx/pull/49)

## [0.3.0](https://github.com/gridaco/fx/tree/v0.3.0)

- Added image generation through the user's Codex installation. [#46](https://github.com/gridaco/fx/pull/46)

## [0.2.1](https://github.com/gridaco/fx/tree/v0.2.1)

- Simplified viewer state indicators and documented `grida fx` as the primary command. [#3](https://github.com/gridaco/fx/pull/3)

## [0.2.0](https://github.com/gridaco/fx/tree/v0.2.0)

- Added the `grida fx` launcher, named runs with live viewing, cancellation, and portable run exports.

## [0.1.0](https://github.com/gridaco/fx/tree/v0.1.0)

- First stable FX release with offline planning, budgeted provider calls, cached runs, SDKs, and an embedded viewer.

## [0.1.0-alpha.2](https://github.com/gridaco/fx/tree/v0.1.0-alpha.2)

- Preview tag for the first stable release.
