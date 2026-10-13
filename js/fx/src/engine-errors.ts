/**
 * Errors a node body sees (`spec/protocol.md` §7).
 *
 * - {@link NodeFailure}: the body fails its node on purpose (`throw ctx.fail("…")`); the host
 *   answers `run` with `node_failure`, which is never retried.
 * - {@link EngineError} and its subclasses: the engine answered one of the run's requests with an
 *   error (codes `-32010` to `-32024`, and `cancelled`). A body may catch them; uncaught, the host
 *   answers `run` with the same code and message, so the engine fails the node with them.
 *   {@link engineError} builds the subclass of a code.
 *
 * Each carries a brand (`Symbol.for`), so the host recognises the errors of any copy of
 * `@grida/fx`, and `instanceof` recognises them across copies too: a project's own `@grida/fx`
 * and the one hosting it may be two.
 */

import { brandOf, INTERNAL } from "./protocol.js";

const NODE_FAILURE_BRAND = Symbol.for("grida.fx.node_failure");
const ENGINE_ERROR_BRAND = Symbol.for("grida.fx.engine_error");

function brand(error: Error, mark: symbol): void {
  Object.defineProperty(error, mark, { value: true });
}

function branded(value: unknown, mark: symbol): boolean {
  return brandOf(value, mark) === true;
}

/** Whether `value` is a {@link NodeFailure} of any copy of `@grida/fx`. */
export function isNodeFailure(value: unknown): value is NodeFailure {
  return branded(value, NODE_FAILURE_BRAND);
}

/** Whether `value` is an {@link EngineError} of any copy of `@grida/fx`. */
export function isEngineError(value: unknown): value is EngineError {
  return branded(value, ENGINE_ERROR_BRAND) && typeof (value as EngineError).code === "number";
}

/** A body fails its node on purpose (`node_failure`, not retried): `throw ctx.fail("…")`. */
export class NodeFailure extends Error {
  constructor(message = "") {
    super(message);
    this.name = new.target.name;
    brand(this, NODE_FAILURE_BRAND);
  }

  static [Symbol.hasInstance](value: unknown): boolean {
    if (Function.prototype[Symbol.hasInstance].call(this, value)) {
      return true;
    }
    return this === NodeFailure && isNodeFailure(value);
  }
}

/** The engine refused a request of the run: `code`, `message`, and `data` when it gave some. */
export class EngineError extends Error {
  /** The code errors of this class carry. */
  static readonly code: number = INTERNAL;
  readonly code: number;
  readonly data: unknown;

  constructor(message: string, data: unknown = null, code?: number) {
    super(message);
    this.name = new.target.name;
    this.code = code ?? (new.target as typeof EngineError).code;
    this.data = data;
    brand(this, ENGINE_ERROR_BRAND);
  }

  static [Symbol.hasInstance](value: unknown): boolean {
    if (Function.prototype[Symbol.hasInstance].call(this, value)) {
      return true;
    }
    if (!isEngineError(value)) {
      return false;
    }
    // Another copy's error: by its code.
    if (this === EngineError) {
      return true;
    }
    if (this === CapabilityError) {
      return value.code >= -32017 && value.code <= -32010;
    }
    return BY_CODE.get(value.code) === this;
  }
}

/** The run was stopped (`cancelled`): a timeout, or a person stopping the run. */
export class Cancelled extends EngineError {
  static override readonly code: number = -32002;
}

/** A paid call that could not be made or answered. */
export class CapabilityError extends EngineError {}

/** `capability_undeclared`: the type does not declare the capability in `calls`. */
export class CapabilityUndeclared extends CapabilityError {
  static override readonly code: number = -32010;
}

/** `over_bound`: the run already made as many calls as the type declared. */
export class OverBound extends CapabilityError {
  static override readonly code: number = -32011;
}

/** `no_route`: no route serves the capability for this instance. */
export class NoRoute extends CapabilityError {
  static override readonly code: number = -32012;
}

/** `not_live`: the call is not cached and the run is not live. */
export class NotLive extends CapabilityError {
  static override readonly code: number = -32013;
}

/** `ceiling_exceeded`: the call's hold does not fit the run's ceiling or a step budget. */
export class CeilingExceeded extends CapabilityError {
  static override readonly code: number = -32014;
}

/** `capability_refused`: the adapter refused before sending; settled at $0. A stand-in throws it
 * to refuse the call it was asked. */
export class CallRefused extends CapabilityError {
  static override readonly code: number = -32015;
}

/** `call_failed`: every attempt failed. A stand-in throws it to fail the call it was asked. */
export class CallFailed extends CapabilityError {
  static override readonly code: number = -32016;
}

/** `job_unsettled`: an earlier submission of this call has an unknown outcome. */
export class JobUnsettled extends CapabilityError {
  static override readonly code: number = -32017;
}

/** `agent_unfinished`: the agent did not answer within `maxSteps` turns. */
export class AgentUnfinished extends EngineError {
  static override readonly code: number = -32020;
}

/** `undeclared_resource`: `ctx.prompt` named a path the type did not declare. */
export class UndeclaredResource extends EngineError {
  static override readonly code: number = -32021;
}

/** `expression_error`: a prompt's expression could not be evaluated. */
export class ExpressionError extends EngineError {
  static override readonly code: number = -32022;
}

/** `outside_work_dir`: a file named by its work path is outside the run's work folder. */
export class OutsideWorkDir extends EngineError {
  static override readonly code: number = -32023;
}

/** `unknown_file`: a file value names a file the engine did not hand this run. */
export class UnknownFile extends EngineError {
  static override readonly code: number = -32024;
}

/** The subclass of each engine code (`spec/protocol.md` §7). */
const BY_CODE: ReadonlyMap<number, typeof EngineError> = new Map(
  [
    Cancelled,
    CapabilityUndeclared,
    OverBound,
    NoRoute,
    NotLive,
    CeilingExceeded,
    CallRefused,
    CallFailed,
    JobUnsettled,
    AgentUnfinished,
    UndeclaredResource,
    ExpressionError,
    OutsideWorkDir,
    UnknownFile,
  ].map((kind) => [kind.code, kind]),
);

/** The {@link EngineError} subclass of `code` (`EngineError` itself for any other code). */
export function engineError(code: number, message: string, data: unknown = null): EngineError {
  const kind = BY_CODE.get(code);
  return kind === undefined ? new EngineError(message, data, code) : new kind(message, data);
}
