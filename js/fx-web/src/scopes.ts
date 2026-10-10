import type { CanvasConnection, CanvasGraph, CanvasPort, CanvasStep, GraphDocument } from "./graph";
import type { ViewerRun } from "./index";
import type { NavigationScope } from "./navigation";
import { scopeState } from "./scope-state";

export interface InterfaceBinding {
  source: string;
  source_port: string;
  source_kind: "output" | "fact" | "scope_input" | "scope_output";
  target_port: string;
}
export interface WorkflowScope {
  id: string;
  parent: string | null;
  kind: "group" | "workflow";
  path: string;
  step: string;
  take: number[];
  title: string;
  source: string | null;
  ports: { inputs: Record<string, unknown>; outputs: string[] };
  input_bindings: InterfaceBinding[];
  output_bindings: InterfaceBinding[];
  nodes: string[];
  pending: string[];
}

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string";
const nonempty = (value: unknown): value is string => text(value) && value.length > 0;
const portName = (value: unknown): value is string => text(value) && /^[a-z][a-z0-9_]*$/.test(value);
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(text) && new Set(value).size === value.length;
const fields = (value: Record<string, unknown>, names: string[]) => Object.keys(value).length === names.length && names.every((name) => name in value);

export function isInterfaceBindings(value: unknown): value is InterfaceBinding[] {
  return Array.isArray(value) && value.every((binding) => record(binding)
    && fields(binding, ["source", "source_port", "source_kind", "target_port"])
    && nonempty(binding.source) && portName(binding.target_port)
    && (binding.source_kind === "fact" ? text(binding.source_port)
      : ["output", "scope_input", "scope_output"].includes(binding.source_kind as string) && portName(binding.source_port)))
    && new Set(value.map((binding) => JSON.stringify([binding.source, binding.source_kind, binding.source_port, binding.target_port]))).size === value.length;
}

export function isWorkflowScopes(value: unknown): value is WorkflowScope[] {
  if (!Array.isArray(value) || !value.every((scope) => record(scope)
    && fields(scope, ["id", "parent", "kind", "path", "step", "take", "title", "source", "ports", "input_bindings", "output_bindings", "nodes", "pending"])
    && text(scope.id) && /^scope:.+#/.test(scope.id) && (scope.parent === null || nonempty(scope.parent))
    && (scope.kind === "group" || scope.kind === "workflow")
    && [scope.path, scope.step].every(nonempty) && text(scope.title) && (scope.source === null || text(scope.source))
    && Array.isArray(scope.take) && scope.take.every((take) => Number.isInteger(take) && take >= 1)
    && record(scope.ports) && fields(scope.ports, ["inputs", "outputs"])
    && record(scope.ports.inputs) && Object.keys(scope.ports.inputs).every(portName)
    && strings(scope.ports.outputs) && scope.ports.outputs.every(portName)
    && isInterfaceBindings(scope.input_bindings) && isInterfaceBindings(scope.output_bindings)
    && strings(scope.nodes) && strings(scope.pending))) return false;
  const scopes = value as WorkflowScope[];
  const byId = new Map(scopes.map((scope) => [scope.id, scope]));
  if (byId.size !== scopes.length) return false;
  const leaves = scopes.flatMap((scope) => scope.nodes);
  const pending = scopes.flatMap((scope) => scope.pending);
  if (new Set(leaves).size !== leaves.length || new Set(pending).size !== pending.length) return false;
  return scopes.every((scope) => {
    const visited = new Set<string>([scope.id]);
    let parent = scope.parent;
    while (parent !== null) {
      if (visited.has(parent)) return false;
      visited.add(parent);
      const ancestor = byId.get(parent);
      if (!ancestor) return false;
      parent = ancestor.parent;
    }
    return true;
  });
}

/** Inline groups contribute frames, while only imported workflows become navigation levels. */
export function navigationScopes(scopes: readonly WorkflowScope[] = []): NavigationScope[] {
  const byId = new Map(scopes.map((scope) => [scope.id, scope]));
  return scopes.filter((scope) => scope.kind === "workflow").map((scope) => {
    let parent = scope.parent;
    const visited = new Set<string>();
    while (parent !== null && !visited.has(parent)) {
      visited.add(parent);
      const ancestor = byId.get(parent);
      if (!ancestor) { parent = null; break; }
      if (ancestor.kind === "workflow") break;
      parent = ancestor.parent;
    }
    return { id: scope.id, title: scope.title || scope.path, parent };
  });
}

function inputPort(name: string, declaration: unknown): CanvasPort {
  if (!record(declaration)) return { name, type: "unknown", kind: "value" };
  const optional = declaration.optional === true ? "?" : "";
  if (declaration.type === "file" || declaration.type === "files") {
    return { name, type: `${typeof declaration.kind === "string" ? declaration.kind : "file"}${declaration.type === "files" ? "[]" : ""}${optional}`, kind: "artifact" };
  }
  const type = typeof declaration.type === "string" ? declaration.type : typeof declaration.$ref === "string" ? "reference" : "object";
  return { name, type: `${type}${optional}`, kind: ["string", "integer", "number", "boolean"].includes(type) ? "parameter" : "value" };
}
const compareName = (a: { name: string }, b: { name: string }) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
function scopePorts(scope: WorkflowScope): NonNullable<CanvasStep["ports"]> {
  return {
    inputs: Object.entries(scope.ports.inputs).map(([name, declaration]) => inputPort(name, declaration)).sort(compareName),
    outputs: scope.ports.outputs.map((name) => ({ name, type: "unknown", kind: "value" as const })).sort(compareName),
    settings: [],
  };
}

/** Visibility projection only. Membership and interface aliases are emitted by the engine. */
export function projectScopes(view: GraphDocument | ViewerRun, flat: CanvasGraph, active: string | null): CanvasGraph {
  if (!view.scopes?.length) return flat;
  const scopes = view.scopes;
  const byId = new Map(scopes.map((scope) => [scope.id, scope]));
  if (active !== null && byId.get(active)?.kind !== "workflow") return flat;
  const plan = view.kind === "fx-graph-v1";
  const allNodes: CanvasStep[] = plan ? flat.nodes : [...flat.nodes, ...scopes.flatMap((scope) => scope.pending).map((path) => ({
    id: `pending:${path}`, source_id: path, title: path, subtitle: "Waiting for repeat expansion", state: "unexpanded", pending: true,
  }))];
  const parents = new Map<string, string | null>();
  for (const scope of scopes) {
    for (const id of scope.nodes) parents.set(`instance:${id}`, scope.id);
    for (const path of scope.pending) parents.set(`pending:${path}`, scope.id);
  }
  const ancestors = (id: string | null): WorkflowScope[] => {
    const chain: WorkflowScope[] = [];
    const visited = new Set<string>();
    while (id !== null && !visited.has(id)) {
      visited.add(id);
      const scope = byId.get(id);
      if (!scope) break;
      chain.unshift(scope);
      id = scope.parent;
    }
    return chain;
  };
  const inActive = (chain: WorkflowScope[]) => active === null || chain.some((scope) => scope.id === active);
  const afterActive = (chain: WorkflowScope[]) => active === null ? chain : chain.slice(chain.findIndex((scope) => scope.id === active) + 1);
  const representative = (id: string): string | null => {
    const chain = ancestors(parents.get(id) ?? null);
    if (!inActive(chain)) return null;
    return afterActive(chain).find((scope) => scope.kind === "workflow")?.id ?? id;
  };
  const visibleScopes = scopes.filter((scope) => {
    const chain = ancestors(scope.parent);
    return inActive(chain) && !afterActive(chain).some((ancestor) => ancestor.kind === "workflow") && scope.id !== active;
  });
  const workflowScopes = visibleScopes.filter((scope) => scope.kind === "workflow");
  const workflowIds = new Set(workflowScopes.map((scope) => scope.id));
  const nodes = allNodes.filter((node) => representative(node.id) === node.id).map((node) => ({ ...node,
    ...(node.ports ? { ports: { inputs: [...node.ports.inputs], outputs: [...node.ports.outputs], settings: [...node.ports.settings] } } : {}),
  }));
  const sourceNodes = new Map((plan ? view.instances : view.nodes).map((node) => [node.id, node]));
  const descendants = (scope: WorkflowScope) => allNodes.filter((node) => ancestors(parents.get(node.id) ?? null).some((ancestor) => ancestor.id === scope.id));
  for (const scope of workflowScopes) {
    const children = descendants(scope);
    const failed = children.filter((node) => node.state === "failed");
    const errors = children.filter((node) => node.state === "failed" || node.state === "blocked").flatMap((node) => {
      const source = sourceNodes.get(node.source_id);
      const message = source && ("error" in source ? source.error : source.reason);
      return message ? [{ id: node.source_id, title: node.title, message }] : [];
    });
    nodes.push({ id: scope.id, source_id: scope.id, kind: "workflow", scope_id: scope.id, path: scope.path, address: scope.step, take: scope.take, key: scope.path,
      title: scope.title || scope.path, subtitle: scope.source ?? scope.path,
      state: scopeState(children.map((node) => node.state), plan), pending: false,
      child_count: children.length, failure_count: failed.length, ...(errors.length ? { errors } : {}), ports: scopePorts(scope) });
  }
  const inputId = active === null ? null : `boundary:input:${active}`;
  const outputId = active === null ? null : `boundary:output:${active}`;
  if (active !== null) {
    const scope = byId.get(active)!;
    const ports = scopePorts(scope);
    if (ports.inputs.length) nodes.unshift({ id: inputId!, source_id: active, kind: "boundary", side: "input", scope_id: active, title: "Workflow inputs", subtitle: scope.title, state: "interface", pending: false, ports: { inputs: [], outputs: ports.inputs, settings: [] } });
    if (ports.outputs.length) nodes.push({ id: outputId!, source_id: active, kind: "boundary", side: "output", scope_id: active, title: "Workflow outputs", subtitle: scope.title, state: "interface", pending: false, ports: { inputs: ports.outputs, outputs: [], settings: [] } });
  }
  const displayed = new Map(nodes.map((node) => [node.id, node]));
  const interfaceBindings = [
    ...(plan ? view.instances : view.nodes).flatMap((node) => node.interface_bindings ?? []),
    ...scopes.flatMap((scope) => [...scope.input_bindings, ...scope.output_bindings]),
  ];
  for (const binding of interfaceBindings) if (binding.source_kind === "fact") {
    const node = displayed.get(`instance:${binding.source}`);
    if (node?.ports && !node.ports.outputs.some((port) => port.kind === "fact" && port.name === binding.source_port)) {
      node.ports.outputs.push({ name: binding.source_port, type: "fact", kind: "fact" });
      node.ports.outputs.sort((a, b) => compareName(a, b) || (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0));
    }
  }
  const edges = new Map<string, CanvasConnection>();
  const add = (edge: CanvasConnection) => {
    if (edge.source === edge.target || !displayed.has(edge.source) || !displayed.has(edge.target)) return;
    edges.set(edge.id, edge);
  };
  const bind = (binding: InterfaceBinding, target: string, targetOwner: string, targetSide: string) => {
    let from: string | null;
    let original: string;
    if (binding.source_kind === "scope_input") {
      from = binding.source === active ? inputId : null;
      original = `boundary:input:${binding.source}`;
    } else if (binding.source_kind === "scope_output") {
      from = workflowIds.has(binding.source) ? binding.source : null;
      original = binding.source;
    } else {
      original = `instance:${binding.source}`;
      from = representative(original);
      // A flattened private port cannot stand in for a public workflow alias.
      if (from !== original) return;
    }
    if (!from || !displayed.get(from)?.ports?.outputs.some((port) => port.name === binding.source_port
      && (binding.source_kind === "fact" ? port.kind === "fact" : binding.source_kind === "output" ? port.kind === "artifact" : true))
      || !displayed.get(target)?.ports?.inputs.some((port) => port.name === binding.target_port)) return;
    const id = binding.source_kind === "output" || binding.source_kind === "fact"
      ? JSON.stringify(["data", original, binding.source_kind, binding.source_port, targetOwner, binding.target_port])
      : JSON.stringify(["interface", original, binding.source_kind, binding.source_port, targetOwner, targetSide, binding.target_port]);
    add({ id, source: from, target, kind: "data", kinds: ["reads"], source_port: binding.source_port, target_port: binding.target_port, source_kind: binding.source_kind === "fact" ? "fact" : "output" });
  };
  const source = plan ? view.instances : view.nodes;
  const authoritative = new Set(source.filter((node) => node.interface_bindings !== undefined).map((node) => `instance:${node.id}`));
  for (const edge of flat.edges) {
    const from = representative(edge.source), to = representative(edge.target);
    if (!from || !to) continue;
    if (edge.kind === "data" && (authoritative.has(edge.target) || from !== edge.source || to !== edge.target)) {
      // Missing interface attribution still proves a dependency, never a public alias.
      // A displayed precise interface connection removes this fallback below.
      add({ id: edge.id, source: from, target: to, kind: "dependency", kinds: ["reads"] });
    } else add({ ...edge, source: from, target: to });
  }
  for (const node of source) {
    const target = `instance:${node.id}`;
    if (!displayed.has(target)) continue;
    for (const binding of node.interface_bindings ?? []) bind(binding, target, target, "node");
  }
  for (const scope of workflowScopes) for (const binding of scope.input_bindings) bind(binding, scope.id, scope.id, "input");
  if (active !== null && outputId !== null) for (const binding of byId.get(active)!.output_bindings) bind(binding, outputId, active, "output");
  const frames = visibleScopes.filter((scope) => scope.kind === "group").map((scope) => ({
    id: scope.id, title: scope.title || scope.path, address: scope.step, take: scope.take, key: scope.path,
    state: scopeState(descendants(scope).map((node) => node.state), plan),
    nodes: nodes.filter((node) => (node.kind === "workflow" ? byId.get(node.scope_id!)?.parent : parents.get(node.id)) === scope.id).map((node) => node.id),
    parent: scope.parent !== active && byId.get(scope.parent ?? "")?.kind === "group" ? scope.parent : null,
  }));
  const nonemptyFrames = new Set(frames.filter((frame) => frame.nodes.length).map((frame) => frame.id));
  const framesById = new Map(frames.map((frame) => [frame.id, frame]));
  for (const id of nonemptyFrames) {
    const parent = framesById.get(id)?.parent;
    if (parent) nonemptyFrames.add(parent);
  }
  const precisePairs = new Set([...edges.values()].filter((edge) => edge.kind === "data").map((edge) => JSON.stringify([edge.source, edge.target])));
  return { nodes, edges: [...edges.values()].filter((edge) => edge.kind !== "dependency" || !precisePairs.has(JSON.stringify([edge.source, edge.target]))), frames: frames.filter((frame) => nonemptyFrames.has(frame.id)) };
}
