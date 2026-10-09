/**
 * Grida FX for JavaScript. The engine is the `grida-fx` command (this package installs it, with
 * the engine package for the machine); this package drives it: plan, price and run workflows,
 * read their runs, and write workflow documents.
 */

export const version = "0.1.0";

export {
  engineVersion,
  expand,
  identity,
  type InspectOptions,
  inspect,
  Plan,
  PlanRefused,
  plan,
  price,
  project,
  RunFile,
  RunResult,
  type RunValue,
  run,
} from "./api.js";
export { BINARY_VARIABLE, type BinaryOptions, binary, type Environment } from "./binary.js";
export { cancel, type CancelOptions, inspectControl, RunControlError, type RunControlResult } from "./control.js";
export {
  loadRun, type ObservedRunEvent, type RunEventBatch, type RunEventsOptions,
  type RunFollowOptions, RunObservationError, type RunObservationFailure,
  type RunReadOptions, RunRecord, type RunSnapshot,
} from "./record.js";
export {
  listRuns, type ListRunsOptions, type RemoveRunsOptions, removeRuns, type RunEntry,
  type RunPlacement, type RunRemoval, type RunRemovalEntry, RunRemovalError,
  type RunRemovalFailure, type RunRemovalSkip, type RunState,
} from "./runs.js";
export type {
  Encoded,
  FileRef,
  Graph,
  GraphInstance,
  Identities,
  InspectedStep,
  Inspection,
  Price,
  PricePhase,
  PriceRange,
  Problem,
  ProjectedInstance,
  Projection,
  RunEvent,
  RunFinished,
  WithValue,
} from "./documents.js";
export type { CallOptions } from "./engine.js";
export { FxError, type FxErrorDetails } from "./errors.js";
export { type Platform, platformFor, platforms } from "./platforms.js";
export type { PlanOptions, RunOptions } from "./request.js";
export {
  type Assertion,
  type Budget,
  type Expression,
  expr,
  type GroupStep,
  type InputDeclaration,
  type InputField,
  type InputRef,
  type JsonValue,
  type KeepBest,
  type NodeStep,
  type Regeneration,
  type Step,
  type WorkflowDocument,
  workflow,
} from "./workflow.js";
export { needsQuotes, toYaml } from "./yaml.js";
