/**
 * Grida FX for JavaScript and TypeScript. The engine is the `grida-fx` command (this package
 * installs it, with the engine package for the machine); this package drives it: plan, price and
 * run workflows, read their runs, and write workflow documents and builders. It also writes node
 * types in TypeScript (`node`, with agent tools made by `tool`), answers paid calls with stand-ins
 * (`run(target, { standIn })`, `Answer`, `DECLINE`), and holds the TypeScript node host the engine
 * starts to run those nodes (`dist/host-entry.js`, Node 24 or later).
 */

export const version = "0.6.1";

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
  RunExportError, type RunExportFailure, type RunExportOptions, type RunExportResult,
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
export type {
  Agent,
  AgentOptions,
  AgentRunOptions,
  CallResult,
  CheckResult,
  Ctx,
  Instance,
  Maker,
  Mark,
  Reader,
  RequestObject,
  RequestValue,
  ToolHandle,
  ToolRun,
  ToolRunOptions,
} from "./ctx.js";
export type { CallOptions } from "./engine.js";
export {
  AgentUnfinished,
  CallFailed,
  CallRefused,
  Cancelled,
  CapabilityError,
  CapabilityUndeclared,
  CeilingExceeded,
  EngineError,
  ExpressionError,
  JobUnsettled,
  NodeFailure,
  NoRoute,
  NotLive,
  OutsideWorkDir,
  OverBound,
  UndeclaredResource,
  UnknownFile,
} from "./engine-errors.js";
export { FxError, type FxErrorDetails } from "./errors.js";
export { InputFile, type Output } from "./files.js";
export {
  type BodyReturn,
  type Family,
  type FromSchema,
  type Inputs,
  type InputsOf,
  type JsonSchemaObject,
  type JsonSchemaType,
  type Kind,
  type NodeDecl,
  type NodeType,
  node,
  type OutputFile,
  type Outputs,
  type OutputsOf,
  type ParamDecl,
  type ParamDecls,
  type Params,
  type ParamSchema,
  type ParamsOf,
  type ParamValue,
  type Picture,
  type Port,
  type Ports,
  type SchemaValue,
  type Tool,
  type ToolArguments,
  type ToolDecl,
  ToolReply,
  type ToolReturn,
  type TypeSpec,
  tool,
} from "./node.js";
export { type Platform, platformFor, platforms } from "./platforms.js";
export type { PlanOptions, RunOptions } from "./request.js";
export {
  Answer,
  type AnswerFile,
  DECLINE,
  type Decline,
  isDecline,
  type StandIn,
  type StandInCall,
  type StandInValue,
  type TurnCall,
} from "./stand-in.js";
export {
  type Assertion,
  type Budget,
  type Expression,
  expr,
  type GroupStep,
  type InputDeclaration,
  type InputField,
  type InputRef,
  type JsonInput,
  type JsonValue,
  type KeepBest,
  type NodeStep,
  type Regeneration,
  type Step,
  type WorkflowDocument,
  workflow,
} from "./workflow.js";
export { needsQuotes, toYaml } from "./yaml.js";
