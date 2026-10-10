import { isNodePorts, isPortBindings, type NodePorts, type PortBinding } from "./ports";
import { isInterfaceBindings, isWorkflowScopes, type InterfaceBinding, type WorkflowScope } from "./scopes";
import { validateViewerApiBase } from "./route";

export interface Artifact {
  digest: string;
  kind: string;
  name: string;
  size: number;
  available: boolean;
  url: string | null;
}

export interface RunNode {
  id: string;
  path: string;
  /** The declared step, when a record names it. */
  step?: string;
  take?: number[];
  key?: string | null;
  /** The run ended while this instance was running. */
  interrupted?: boolean;
  title: string;
  uses: string | null;
  state: string;
  reads: string[];
  with: Record<string, unknown>;
  outputs: Record<string, unknown>;
  cache: string | null;
  error: string | null;
  duration_ms: number | null;
  /** When this instance last started, UTC; absent when the record cannot date it. */
  started_at?: string;
  ports?: NodePorts;
  bindings?: PortBinding[];
  needs?: string[];
  judges?: string | null;
  interface_bindings?: InterfaceBinding[];
}

export interface ViewerRun {
  kind: "fx-viewer-run-v1";
  workflow: { id: string; title: string; description?: string };
  run_name: string;
  state: string;
  stand_in: boolean;
  charged_usd: number | null;
  estimate: { low_usd: number; high_usd: number } | null;
  inputs: Record<string, unknown>;
  outputs: Record<string, unknown>;
  nodes: RunNode[];
  artifacts: Artifact[];
  warnings: string[];
  scopes?: WorkflowScope[];
  /** The repeats not yet expanded. */
  pending?: { path: string; step?: string; max: number; phase: number; waiting_on?: string[] }[];
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function strings(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function nullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

/** A real UTC calendar time in the records' RFC3339 form. */
export function utcTimestamp(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?Z$/.exec(value);
  if (!match) return false;
  const date = new Date(value);
  return Number.isFinite(date.getTime())
    && [date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), date.getUTCHours(), date.getUTCMinutes(), date.getUTCSeconds()]
      .every((part, index) => part === Number(match[index + 1]));
}

function nullableNumber(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isFinite(value));
}

function node(value: unknown): value is RunNode {
  return record(value)
    && [value.id, value.path, value.title, value.state].every((item) => typeof item === "string")
    && nullableString(value.uses)
    && (value.step === undefined || typeof value.step === "string")
    && (value.interrupted === undefined || typeof value.interrupted === "boolean")
    && strings(value.reads)
    && record(value.with)
    && record(value.outputs)
    && nullableString(value.cache)
    && nullableString(value.error)
    && nullableNumber(value.duration_ms)
    && (value.started_at === undefined || utcTimestamp(value.started_at))
    && (value.ports === undefined || isNodePorts(value.ports))
    && (value.bindings === undefined || isPortBindings(value.bindings))
    && (value.needs === undefined || strings(value.needs))
    && (value.judges === undefined || nullableString(value.judges))
    && (value.interface_bindings === undefined || isInterfaceBindings(value.interface_bindings));
}

function artifact(value: unknown, apiBase: string): value is Artifact {
  return record(value)
    && typeof value.digest === "string" && /^[a-f0-9]{64}$/.test(value.digest)
    && typeof value.kind === "string"
    && typeof value.name === "string"
    && typeof value.size === "number" && Number.isFinite(value.size) && value.size >= 0
    && typeof value.available === "boolean"
    && nullableString(value.url)
    && (value.url === null || value.url === `${apiBase}/artifacts/${value.digest}`);
}

/** Validate the browser boundary before any artifact URL is used. */
export function parseViewerRun(value: unknown, apiBase = "/api"): ViewerRun {
  validateViewerApiBase(apiBase);
  if (!record(value) || value.kind !== "fx-viewer-run-v1") {
    throw new Error("This viewer does not support the returned run format.");
  }
  const valid = record(value.workflow)
    && typeof value.workflow.id === "string"
    && typeof value.workflow.title === "string"
    && (value.workflow.description === undefined || typeof value.workflow.description === "string")
    && typeof value.run_name === "string"
    && typeof value.state === "string"
    && typeof value.stand_in === "boolean"
    && nullableNumber(value.charged_usd)
    && (value.estimate === null || (record(value.estimate)
      && typeof value.estimate.low_usd === "number"
      && typeof value.estimate.high_usd === "number"))
    && record(value.inputs) && record(value.outputs)
    && Array.isArray(value.nodes) && value.nodes.every(node)
    && Array.isArray(value.artifacts) && value.artifacts.every((item) => artifact(item, apiBase))
    && strings(value.warnings)
    && (value.scopes === undefined || isWorkflowScopes(value.scopes));
  if (!valid) throw new Error("The run response is incomplete or malformed.");
  return value as unknown as ViewerRun;
}

export async function readRun(signal?: AbortSignal, apiBase = "/api"): Promise<ViewerRun> {
  const response = await fetch(`${validateViewerApiBase(apiBase)}/run`, { signal, cache: "no-store" });
  if (!response.ok) throw new Error(`The run could not be read (HTTP ${response.status}).`);
  return parseViewerRun(await response.json(), apiBase);
}

export type ViewerView = GraphDocument | ViewerRun;

export function parseViewerView(value: unknown, apiBase = "/api"): ViewerView {
  validateViewerApiBase(apiBase);
  if (record(value) && value.kind === "fx-graph-v1") return parseGraph(value);
  return parseViewerRun(value, apiBase);
}

export async function readView(signal?: AbortSignal, apiBase = "/api"): Promise<ViewerView> {
  const response = await fetch(`${validateViewerApiBase(apiBase)}/view`, { signal, cache: "no-store" });
  if (!response.ok) throw new Error(`The workflow could not be read (HTTP ${response.status}).`);
  return parseViewerView(await response.json(), apiBase);
}

/** Find encoded FX file references without interpreting workflow expressions. */
export function artifactDigests(value: unknown): string[] {
  const found = new Set<string>();
  const visit = (item: unknown) => {
    if (Array.isArray(item)) {
      item.forEach(visit);
    } else if (record(item)) {
      if (record(item.file) && typeof item.file.digest === "string") {
        found.add(item.file.digest);
      } else if (typeof item.file === "string" && /^[a-f0-9]{64}$/.test(item.file)) {
        found.add(item.file);
      }
      Object.values(item).forEach(visit);
    }
  };
  visit(value);
  return [...found];
}
import { parseGraph, type GraphDocument } from "./graph";
export * from "./graph";
export * from "./controller";
export * from "./canvas";
export * from "./layout";
export * from "./deck";
export * from "./ports";
export * from "./navigation";
export * from "./scopes";
export * from "./observation";
export * from "./route";
export * from "./service";
