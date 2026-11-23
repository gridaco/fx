# Changelog

Notable changes to Grida FX are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and version numbers use
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Starting with 0.11.0, each [GitHub Release](https://github.com/gridaco/fx/releases)
publishes the corresponding notes and links to the packages on npm and PyPI.

The initial backfill lists selected highlights and uses Git tag dates in Asia/Seoul,
with the tagged commit date for the lightweight `v0.4.0` tag.

## [Unreleased]

### Added

- Pause an active workflow with `grida fx pause RUN`, which lets running steps finish before saving a durable boundary. Continue the same attempt with `grida fx resume RUN` using the recorded working directory. The CLI, SDKs, and viewer expose when the run is pausing or paused. Run removal filters accept both states. [#166](https://github.com/gridaco/fx/issues/166)
- Start a fresh attempt of a recorded run with `grida fx rerun RUN`. The run keeps its ID, and `grida fx inspect RUN --history --json` lists its attempts. Earlier outputs and reviews remain addressable under their exact attempts. [#162](https://github.com/gridaco/fx/issues/162)

### Changed

- A run name selects its latest attempt. To keep a script on an earlier result, use the exact attempt folder or inspect the run with `--attempt N`. [#162](https://github.com/gridaco/fx/issues/162)

### Fixed

- Restore keyboard focus after adding or removing a row in a workflow review form.

## [0.12.0] - 2026-11-22

### Added

- Download a standalone `grida-fx` executable for macOS or Linux from GitHub Releases, with a checksum for each supported target. [#156](https://github.com/gridaco/fx/issues/156)
- Workflows can request external input with `fx/await@1` and `with.schema`. An SDK can supply a promptless value, or a person can review a question and select an exact file. The viewer renders scalar enums as pickers, marks pending requests on the run and project home, and counts down to an optional deadline. The CLI and SDKs can list and answer requests without a viewer. [#62](https://github.com/gridaco/fx/issues/62)
- Waiting steps stand out on the canvas and open their form from a **Review** button. Resolved steps show the outcome and open a receipt with the answer, submission time, and channel.

### Changed

- Made `llms.txt` a shorter starting point for agents, with an introduction to FX, runnable recipes, and links organized by task.
- The FX Viewer favicon pulses green during active execution and stays steady with reduced motion. [#158](https://github.com/gridaco/fx/issues/158)
- A run awaiting answers shows a steady amber favicon and its pending review count in the tab title.

### Fixed

- Resume collecting a completed provider job after a failed download or validation without paying for generation again. Use `--recover` or the Python and JavaScript `recover` option to retry collection without authorizing new calls, including on runs from older versions that kept the task handle. [#116](https://github.com/gridaco/fx/issues/116)

## [0.11.0] - 2026-11-20

### Added

- Register a plan or saved run with `--register` to get its viewer route without launching a browser. Host apps can resolve the route and files using the exact project. [#144](https://github.com/gridaco/fx/pull/144)
- Copy a private GitHub template with `grida fx add OWNER/REPO@REF --github-auth` using your existing GitHub CLI authentication. [#152](https://github.com/gridaco/fx/pull/152)
- Generate one image from the terminal with `grida fx call image.generate`. Use a project route or select one explicitly, check it with `--dry-run`, and get a PNG and a normal run record. [#150](https://github.com/gridaco/fx/pull/150)

### Changed

- For provider failures, `grida fx run` points to the exact run and shows a safe explanation when available. `inspect` and both SDKs show saved failed attempts, including failures recovered by retry. Codex failures include ephemeral thread and turn IDs when available. [#148](https://github.com/gridaco/fx/pull/148)
- Improved browser tab metadata and discovery of FX Viewer documents for host apps. [#145](https://github.com/gridaco/fx/pull/145)

## [0.10.0] - 2026-11-19

### Added

- Start from editable workflow templates with `grida fx init` and `add`, and use provider credentials saved in Grida. [#143](https://github.com/gridaco/fx/pull/143), [#140](https://github.com/gridaco/fx/pull/140)

## [0.9.0] - 2026-11-18

### Added

- Choose models and providers separately, discover compatible image models, and explore recorded workflow values in the viewer. [#132](https://github.com/gridaco/fx/pull/132), [#133](https://github.com/gridaco/fx/pull/133), [#139](https://github.com/gridaco/fx/pull/139)

## [0.8.1] - 2026-11-12

### Added

- Added an embedded viewer browser client, single-image Tripo mesh generation, and structured provider diagnostics. [#127](https://github.com/gridaco/fx/pull/127), [#124](https://github.com/gridaco/fx/pull/124)

## [0.8.0] - 2026-11-11

### Added

- Added configurable OpenAI-compatible requests and billing, plus author-selected video posters. [#109](https://github.com/gridaco/fx/pull/109), [#120](https://github.com/gridaco/fx/pull/120)

## [0.7.1] - 2026-10-15

### Fixed

- Reissued 0.7.0 to unblock npm installation on Linux x64. [#108](https://github.com/gridaco/fx/pull/108)

## [0.7.0] - 2026-10-15

### Added

- Added project-declared providers and adapters, more flexible model routes, and per-call caps. [#106](https://github.com/gridaco/fx/pull/106)

## [0.6.1] - 2026-10-14

### Added

- Added Grok Build image generation and video previews in the viewer. [#76](https://github.com/gridaco/fx/pull/76), [#78](https://github.com/gridaco/fx/pull/78)

## [0.6.0] - 2026-10-13

### Added

- Added TypeScript nodes, workflow builders, and stand-ins. [#71](https://github.com/gridaco/fx/pull/71)

## [0.5.1] - 2026-10-12

### Added

- Added project starters, examples, and skills through `grida fx init`. [#59](https://github.com/gridaco/fx/pull/59)

## [0.5.0] - 2026-10-12

### Added

- Added Codex reference images, planning-time request checks, and visible waiting states. [#58](https://github.com/gridaco/fx/pull/58)

## [0.4.0] - 2026-10-12

### Added

- Added Grida Gateway as a provider. [#49](https://github.com/gridaco/fx/pull/49)

## [0.3.0] - 2026-10-12

### Added

- Added image generation through the user's Codex installation. [#46](https://github.com/gridaco/fx/pull/46)

## [0.2.1] - 2026-10-12

### Changed

- Simplified viewer state indicators and documented `grida fx` as the primary command. [#3](https://github.com/gridaco/fx/pull/3)

## [0.2.0] - 2026-10-11

### Added

- Added the `grida fx` launcher, named runs with live viewing, cancellation, and portable run exports.

## [0.1.0] - 2026-10-08

### Added

- First stable FX release with offline planning, budgeted provider calls, cached runs, SDKs, and an embedded viewer.

## [0.1.0-alpha.2] - 2026-10-07

### Added

- Preview tag for the first stable release.

[Unreleased]: https://github.com/gridaco/fx/compare/v0.12.0...HEAD
[0.12.0]: https://github.com/gridaco/fx/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/gridaco/fx/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/gridaco/fx/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/gridaco/fx/compare/v0.8.1...v0.9.0
[0.8.1]: https://github.com/gridaco/fx/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/gridaco/fx/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/gridaco/fx/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/gridaco/fx/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/gridaco/fx/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/gridaco/fx/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/gridaco/fx/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/gridaco/fx/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/gridaco/fx/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/gridaco/fx/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/gridaco/fx/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/gridaco/fx/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/gridaco/fx/compare/v0.1.0-alpha.2...v0.1.0
[0.1.0-alpha.2]: https://github.com/gridaco/fx/tree/v0.1.0-alpha.2
