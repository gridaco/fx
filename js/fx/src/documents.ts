/**
 * The JSON documents the engine prints, as TypeScript types: written by hand from `spec/schemas/`
 * (fx-graph-v1, fx-run-events-v1) and the command line (`price`, `identity`, `project`,
 * `inspect --json`). Their fields keep the contracts' `lower_snake_case`. They cover what a
 * program reads; the schemas are complete.
 */

import type { JsonValue } from "./workflow.js";

/** A problem that refuses a plan. */
export interface Problem {
  where: string;
  message: string;
}

/** An amount range in US dollars. */
export interface PriceRange {
  low_usd: number;
  high_usd: number;
}

/** A with-value of the graph: a plain value, or `{ pending: [<instance ids>] }` while it waits on
 * what runs before it. */
export type WithValue = JsonValue;

/** One step instance of the expanded graph. */
export interface GraphInstance {
  /** The instance id, e.g. `draw#1` or `entity['ada'].draw#1`. */
  id: string;
  /** The step path, with repeat keys. */
  path: string;
  /** The declared step's path, without repeat keys. */
  step: string;
  /** One take per regenerating level, outermost first. */
  take: number[];
  uses: string;
  /** The type identity; null when the type could not be resolved. */
  type: string | null;
  with: { [name: string]: WithValue };
  /** Each capability the type calls, with the route bound to it. */
  routes: { [capability: string]: { route: string; fingerprint: string } };
  state: "planned" | "maybe" | "absent" | "blocked" | "failed" | "done";
  /** The step identity; null while a with-value is pending. */
  identity: string | null;
  phase: number;
  key: string | null;
  judges: string | null;
  judged_by: string[];
  waiting_on: string[];
  needs: string[];
  reads: string[];
  price: PriceRange;
  view?: boolean | string;
  [field: string]: unknown;
}

/** `grida-fx expand`, and `plan --json`: the expanded graph (fx-graph-v1). */
export interface Graph {
  kind: "fx-graph-v1";
  workflow: { id: string; title: string; description?: string; file?: string };
  /** Each distinct `uses`, with its type identity (and a project type's source digests). */
  types?: {
    [uses: string]: {
      identity: string;
      source?: { files: { [label: string]: string }; resources: { [path: string]: string } };
    };
  };
  instances: GraphInstance[];
  /** Repeats whose instances a later phase expands, with their worst case. */
  pending: { path: string; max: number; phase: number; high_usd: number }[];
  estimate: PriceRange & { ceiling_usd: number | null };
  /** Why the plan is refused; empty when it is not. */
  problems: Problem[];
  [field: string]: unknown;
}

/** `grida-fx identity`: each live instance's identity, null while it is pending. */
export type Identities = { [instanceId: string]: string | null };

/** One phase of {@link Price}. */
export interface PricePhase extends PriceRange {
  phase: number;
  steps: number;
  /** The fewest and the most provider calls. */
  calls: [number, number];
  /** What the phase leaves to later phases (repeats over a run-time list). */
  then: string[];
}

/** `grida-fx price`: the plan's price by phase. */
export interface Price {
  phases: PricePhase[];
  estimate: PriceRange;
  ceiling_usd: number | null;
}

/** A file in a run's record: by digest. */
export interface FileRef {
  digest: string;
  /** The file kind (`image/png`, `json`, …). */
  kind: string;
  name: string;
  size: number;
  /** A collection item's key. */
  key?: string;
}

/** A value in a run's record (fx-run-events-v1 `encoded`). */
export type Encoded =
  | { file: FileRef }
  | { collection: [string, Encoded][] }
  | { list: Encoded[] }
  | { none: true }
  | { value: JsonValue };

/** One event of a run's `events.jsonl` (fx-run-events-v1), without `kind`; only the common fields
 * are typed. */
export interface RunEvent {
  event: string;
  plan?: string;
  [field: string]: unknown;
}

/** The `run_finished` event. */
export interface RunFinished extends RunEvent {
  event: "run_finished";
  ok: boolean;
  incomplete: boolean;
  stopped: string | null;
  charged_usd: number;
  failed: string[];
  outputs: { [name: string]: Encoded };
}

/** One instance in {@link Projection}. */
export interface ProjectedInstance {
  state: "running" | "succeeded" | "failed" | "skipped";
  path: string;
  /** Succeeded: whether its result came from the cache. */
  cache?: "hit" | "miss" | null;
  /** Succeeded: the facts its body reported. */
  facts?: { [name: string]: JsonValue };
  /** Failed or skipped: why. */
  error?: string | null;
}

/** `grida-fx project <run>`: a run's record projected to its state. */
export interface Projection {
  instances: { [instanceId: string]: ProjectedInstance };
  run: {
    run_started?: RunEvent;
    run_finished?: RunFinished;
    run_cancelled?: RunEvent & { reason?: string; charged_usd?: number };
  };
}

/** One step of {@link Inspection}. */
export interface InspectedStep {
  id: string;
  path: string;
  state: "succeeded" | "failed" | "skipped" | "running" | "pending";
  error?: string;
  cache?: "hit" | "miss";
  /** Its files, where the run placed them: paths relative to the run folder. */
  files: { path: string; digest: string; size: number }[];
}

/** `grida-fx inspect <run> --json`: a run's summary. */
export interface Inspection {
  run: {
    workflow: string;
    /** The run folder, as it was named. */
    folder: string;
    state: "planned" | "unfinished" | "succeeded" | "failed" | "cancelled";
    /** What the run was charged; null before it ended. */
    charged_usd: number | null;
    steps: InspectedStep[];
  };
  /** With `verify`: whether every placed file still matches its record. */
  verification?: { verified: boolean; problems: string[] };
}
