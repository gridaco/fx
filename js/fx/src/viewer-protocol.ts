/** The browser-only viewer page contract (spec/viewer.md). */
export namespace ViewerProtocol {
  export const CHANNEL = "fx_viewer";
  export const VERSION = 1;

  export type Reference =
    | { kind: "service_entry"; project_id: string; entry_kind: "plan" | "run"; entry_id: string }
    | { kind: "export"; manifest_sha256: string };
  export interface Subject {
    kind: "plan" | "run" | "catalog";
    workflow?: { id: string; source?: string };
    reference?: Reference;
  }
  export type Selection =
    | { kind: "instance"; instance_id: string; path?: string; step?: string; take?: number[] }
    | { kind: "scope"; scope_id: string }
    | { kind: "pending_repeat"; path: string }
    | { kind: "scope_boundary"; scope_id: string; side: "input" | "output" };
  export interface Context {
    subject: Subject;
    scope_id: string | null;
    selection: Selection | null;
    sampled_at: number;
    retained?: true;
  }
  export type Action =
    | { action: "reveal_instance"; instance_id: string }
    | { action: "open_scope"; scope_id: string | null }
    | { action: "clear_selection" }
    | { action: "fit" };
  export type Method = "get_context" | "subscribe" | "unsubscribe" | "present";
  /** Unknown methods/versions remain recognizable so a page can refuse them explicitly. */
  export interface Request {
    channel: typeof CHANNEL;
    version: number;
    type: "request";
    request_id: string;
    method: string;
    params: Record<string, unknown>;
  }
  export interface Error {
    code: string;
    message: string;
  }
  interface ResponseBase {
    channel: typeof CHANNEL;
    version: typeof VERSION;
    type: "response";
    request_id: string;
  }
  export type Response = ResponseBase & (
    | { ok: true; result: Context | null }
    | { ok: false; error: Error }
  );
  export interface ContextEvent {
    channel: typeof CHANNEL;
    version: typeof VERSION;
    type: "context_changed";
    context: Context;
  }

  export function record(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }
  function fields(value: Record<string, unknown>, required: readonly string[], optional: readonly string[] = []): boolean {
    return required.every((key) => Object.hasOwn(value, key))
      && Object.keys(value).every((key) => required.includes(key) || optional.includes(key));
  }
  export function isIdentifier(value: unknown): value is string {
    return typeof value === "string" && value.length > 0 && !/[\u0000-\u001f\u007f]/.test(value);
  }
  /** The same portable source boundary used by the project catalog. */
  export function isPortableSource(value: unknown): value is string {
    return isIdentifier(value) && !/^[\\/]|^[A-Za-z]:|^file:|:\/\/|\\/i.test(value);
  }
  function digest(value: unknown): value is string {
    return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
  }
  export function isReference(value: unknown): value is Reference {
    if (!record(value)) return false;
    if (value.kind === "export") return fields(value, ["kind", "manifest_sha256"]) && digest(value.manifest_sha256);
    return value.kind === "service_entry" && fields(value, ["kind", "project_id", "entry_kind", "entry_id"])
      && digest(value.project_id) && digest(value.entry_id) && (value.entry_kind === "plan" || value.entry_kind === "run");
  }
  export function isSubject(value: unknown): value is Subject {
    if (!record(value) || !fields(value, ["kind"], ["workflow", "reference"])
      || (value.kind !== "plan" && value.kind !== "run" && value.kind !== "catalog")) return false;
    if (Object.hasOwn(value, "workflow")) {
      const workflow = value.workflow;
      if (!record(workflow) || !fields(workflow, ["id"], ["source"]) || !isIdentifier(workflow.id)
        || (Object.hasOwn(workflow, "source") && !isPortableSource(workflow.source))) return false;
    }
    if (Object.hasOwn(value, "reference")) {
      if (!isReference(value.reference)) return false;
      if (value.reference.kind === "service_entry" && value.reference.entry_kind !== value.kind) return false;
      if (value.reference.kind === "export" && value.kind !== "run") return false;
    }
    return true;
  }
  export function isSelection(value: unknown): value is Selection {
    if (!record(value)) return false;
    switch (value.kind) {
      case "instance":
        return fields(value, ["kind", "instance_id"], ["path", "step", "take"])
          && isIdentifier(value.instance_id)
          && ["path", "step"].every((key) => !Object.hasOwn(value, key) || isIdentifier(value[key]))
          && (!Object.hasOwn(value, "take") || Array.isArray(value.take) && value.take.length > 0
            && value.take.every((take) => Number.isSafeInteger(take) && take > 0));
      case "scope":
        return fields(value, ["kind", "scope_id"]) && isIdentifier(value.scope_id);
      case "pending_repeat":
        return fields(value, ["kind", "path"]) && isIdentifier(value.path);
      case "scope_boundary":
        return fields(value, ["kind", "scope_id", "side"]) && isIdentifier(value.scope_id)
          && (value.side === "input" || value.side === "output");
      default:
        return false;
    }
  }
  export function isContext(value: unknown): value is Context {
    return record(value) && fields(value, ["subject", "scope_id", "selection", "sampled_at"], ["retained"])
      && isSubject(value.subject) && (value.scope_id === null || isIdentifier(value.scope_id))
      && (value.selection === null || isSelection(value.selection))
      && typeof value.sampled_at === "number" && Number.isSafeInteger(value.sampled_at) && value.sampled_at >= 0
      && (!Object.hasOwn(value, "retained") || value.retained === true);
  }
  export function isAction(value: unknown): value is Action {
    if (!record(value)) return false;
    switch (value.action) {
      case "reveal_instance": return fields(value, ["action", "instance_id"]) && isIdentifier(value.instance_id);
      case "open_scope": return fields(value, ["action", "scope_id"]) && (value.scope_id === null || isIdentifier(value.scope_id));
      case "clear_selection":
      case "fit": return fields(value, ["action"]);
      default: return false;
    }
  }
  export function isMethod(value: unknown): value is Method {
    return typeof value === "string" && ["get_context", "subscribe", "unsubscribe", "present"].includes(value);
  }
  export function isParams(method: string, value: unknown): boolean {
    if (!isMethod(method) || !record(value)) return false;
    return method === "present" ? isAction(value) : Object.keys(value).length === 0;
  }
  export function isRequest(value: unknown): value is Request {
    return record(value) && value.channel === CHANNEL && Number.isSafeInteger(value.version)
      && value.type === "request" && isIdentifier(value.request_id) && isIdentifier(value.method) && record(value.params);
  }
  export function isResponse(value: unknown): value is Response {
    if (!record(value) || value.channel !== CHANNEL || value.version !== VERSION
      || value.type !== "response" || !isIdentifier(value.request_id)) return false;
    if (value.ok === true) return Object.hasOwn(value, "result") && !Object.hasOwn(value, "error")
      && (value.result === null || isContext(value.result));
    return value.ok === false && !Object.hasOwn(value, "result") && record(value.error)
      && isIdentifier(value.error.code) && typeof value.error.message === "string";
  }
  export function isContextEvent(value: unknown): value is ContextEvent {
    return record(value) && value.channel === CHANNEL && value.version === VERSION
      && value.type === "context_changed" && isContext(value.context);
  }
}
