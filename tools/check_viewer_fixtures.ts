#!/usr/bin/env bun
/** Check generated FX viewer harness plans and host-derived run views without a browser.
 *
 *   bun --no-env-file tools/check_viewer_fixtures.ts .fx/viewer-fixtures
 *
 * Only expected.json and its explicitly listed plans/*.json and views/*.json are read.
 * The contributor harness does not read credentials, execute workflows, or change artifacts.
 */
import { lstatSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  canvasGraph,
  flatCanvasGraph,
  canvasPortAnchor,
  layoutGraph,
  parseViewerView,
  ViewerController,
  type CanvasGraph,
  type RunNode,
  type ViewerView,
} from "../js/fx-web/src/index";

interface EdgeExpectation { source: string; target: string; kinds?: string[]; kind?: string }
interface PortsExpectation {
  id: string;
  inputs?: Record<string, string>;
  outputs?: Record<string, string>;
  params?: string[];
}
interface BindingExpectation {
  source: string; target: string; source_port: string; target_port: string;
  source_kind: "output" | "fact";
}
interface ScopeExpectation {
  path: string; parent: string | null; kind: "workflow" | "group";
  source?: string | null; inputs?: string[]; outputs?: string[]; nodes?: string[];
  state?: string; visible_count?: number;
}
interface InterfaceExpectation {
  source: string; source_port: string;
  source_kind: "output" | "fact" | "scope_input" | "scope_output";
  target: string; target_port: string; target_kind: "node" | "scope_input" | "scope_output";
}
interface CaseExpectation {
  file: string;
  node_count?: number;
  edge_count?: number;
  pending_count?: number;
  preview_count?: number;
  state?: string;
  cache_hits?: number;
  requires_nodes?: string[];
  required_states?: string[];
  requires_edges?: EdgeExpectation[];
  requires_ports?: PortsExpectation[];
  requires_bindings?: BindingExpectation[];
  literal_settings?: { id: string; name: string }[];
  legacy?: boolean;
  requires_scopes?: ScopeExpectation[];
  requires_interfaces?: InterfaceExpectation[];
  root_node_count?: number;
}
interface Expectations {
  kind: "fx-viewer-fixtures-v1";
  plans: CaseExpectation[];
  views: CaseExpectation[];
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function stringList(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}
function checkDirectory(path: string) {
  const stat = lstatSync(path);
  assert(stat.isDirectory() && !stat.isSymbolicLink(), `${path}: expected a regular directory`);
}
function readJson(directory: string, file: string): unknown {
  assert(/^[a-z0-9][a-z0-9._-]*\.json$/.test(file), `Invalid fixture filename: ${file}`);
  const path = join(directory, file);
  const stat = lstatSync(path);
  assert(stat.isFile() && !stat.isSymbolicLink(), `${file}: expected a regular JSON file`);
  return JSON.parse(readFileSync(path, "utf8"));
}
function cases(value: unknown, section: string): asserts value is CaseExpectation[] {
  assert(Array.isArray(value) && value.length > 0, `expected.json: ${section} must list at least one case`);
  const files = new Set<string>();
  for (const item of value) {
    assert(record(item) && typeof item.file === "string", `expected.json: ${section} case needs a filename`);
    assert(!files.has(item.file), `expected.json: duplicate ${section}/${item.file}`);
    files.add(item.file);
    for (const field of ["node_count", "edge_count", "pending_count", "preview_count", "cache_hits", "root_node_count"]) {
      if (!(field in item)) continue;
      assert(typeof item[field] === "number" && Number.isInteger(item[field]) && item[field] >= 0, `${item.file}: ${field} must be a nonnegative integer`);
    }
    if ("state" in item) assert(typeof item.state === "string", `${item.file}: state must be a string`);
    if ("legacy" in item) assert(typeof item.legacy === "boolean", `${item.file}: legacy must be a boolean`);
    for (const field of ["requires_nodes", "required_states"]) {
      if (field in item) assert(stringList(item[field]), `${item.file}: ${field} must contain strings`);
    }
    if ("requires_edges" in item) {
      assert(Array.isArray(item.requires_edges), `${item.file}: requires_edges must be an array`);
      for (const edge of item.requires_edges) {
        assert(record(edge) && typeof edge.source === "string" && typeof edge.target === "string", `${item.file}: an edge needs source and target instance IDs`);
        if ("kinds" in edge) assert(stringList(edge.kinds), `${item.file}: edge kinds must contain strings`);
        if ("kind" in edge) assert(["data", "control", "dependency"].includes(edge.kind as string), `${item.file}: unsupported canvas edge kind`);
      }
    }
    if ("requires_ports" in item) {
      assert(Array.isArray(item.requires_ports), `${item.file}: requires_ports must be an array`);
      for (const ports of item.requires_ports) {
        assert(record(ports) && typeof ports.id === "string", `${item.file}: port metadata needs an instance ID`);
        for (const field of ["inputs", "outputs"]) {
          if (field in ports) assert(record(ports[field]) && Object.values(ports[field]).every((value) => typeof value === "string"), `${item.file}: ${field} must map names to port notation`);
        }
        if ("params" in ports) assert(stringList(ports.params), `${item.file}: params must list setting names`);
      }
    }
    if ("requires_bindings" in item) {
      assert(Array.isArray(item.requires_bindings), `${item.file}: requires_bindings must be an array`);
      for (const binding of item.requires_bindings) {
        assert(record(binding) && [binding.source, binding.target, binding.source_port, binding.target_port].every((value) => typeof value === "string") && ["output", "fact"].includes(binding.source_kind as string), `${item.file}: malformed binding expectation`);
      }
    }
    if ("literal_settings" in item) {
      assert(Array.isArray(item.literal_settings) && item.literal_settings.every((setting) => record(setting) && typeof setting.id === "string" && typeof setting.name === "string"), `${item.file}: literal_settings must name instances and settings`);
    }
    if ("requires_scopes" in item) {
      assert(Array.isArray(item.requires_scopes), `${item.file}: requires_scopes must be an array`);
      for (const scope of item.requires_scopes) {
        assert(record(scope) && typeof scope.path === "string" && (scope.parent === null || typeof scope.parent === "string") && ["workflow", "group"].includes(scope.kind as string), `${item.file}: malformed scope expectation`);
        for (const field of ["inputs", "outputs", "nodes"]) if (field in scope) assert(stringList(scope[field]), `${item.file}: scope ${field} must list names`);
        for (const field of ["source", "state"]) if (field in scope) assert(scope[field] === null || typeof scope[field] === "string", `${item.file}: scope ${field} must be a string or null`);
        if ("visible_count" in scope) assert(typeof scope.visible_count === "number" && Number.isInteger(scope.visible_count) && scope.visible_count >= 0, `${item.file}: invalid scope visible_count`);
      }
    }
    if ("requires_interfaces" in item) {
      assert(Array.isArray(item.requires_interfaces), `${item.file}: requires_interfaces must be an array`);
      for (const binding of item.requires_interfaces) {
        assert(record(binding) && [binding.source, binding.source_port, binding.target, binding.target_port].every((value) => typeof value === "string") && ["output", "fact", "scope_input", "scope_output"].includes(binding.source_kind as string) && ["node", "scope_input", "scope_output"].includes(binding.target_kind as string), `${item.file}: malformed interface binding expectation`);
      }
    }
  }
}
function readExpectations(root: string): Expectations {
  const value = readJson(root, "expected.json");
  assert(record(value) && value.kind === "fx-viewer-fixtures-v1", "expected.json: unsupported fixture contract");
  cases(value.plans, "plans");
  cases(value.views, "views");
  return value as unknown as Expectations;
}
function count(expected: number | undefined, actual: number, label: string) {
  if (expected !== undefined) assert(actual === expected, `${label}: expected ${expected}, received ${actual}`);
}

/** Encoded JSON values are opaque; only file/list/collection variants carry file references. */
function outputFiles(value: unknown): string[] {
  if (!record(value) || "value" in value) return [];
  if (record(value.file) && typeof value.file.digest === "string") return [value.file.digest];
  if (Array.isArray(value.list)) return value.list.flatMap(outputFiles);
  if (Array.isArray(value.collection)) return value.collection.flatMap((entry) => Array.isArray(entry) && entry.length === 2 ? outputFiles(entry[1]) : []);
  return [];
}
function checkImages(view: ViewerView, graph: CanvasGraph, label: string) {
  if (view.kind === "fx-graph-v1") {
    assert(graph.nodes.every((node) => !node.preview), `${label}: a static plan must not show output image previews`);
    return;
  }
  const artifacts = new Map(view.artifacts.map((artifact) => [artifact.digest, artifact]));
  const imageKinds = new Set(["image/png", "image/jpeg", "image/webp", "image/gif", "image/avif"]);
  for (const node of view.nodes) {
    const projected = graph.nodes.find((item) => item.source_id === node.id);
    assert(projected, `${label}/${node.id}: recorded node is missing from the canvas`);
    const eligible = [...new Set(Object.values(node.outputs).flatMap(outputFiles))].filter((digest) => {
      const artifact = artifacts.get(digest);
      return artifact?.available && imageKinds.has(artifact.kind) && artifact.url === `/api/artifacts/${digest}`;
    });
    if (!eligible.length) {
      assert(!projected.preview, `${label}/${node.id}: preview invented for unavailable or nonimage output`);
      continue;
    }
    const preview = projected.preview;
    assert(preview?.kind === "image", `${label}/${node.id}: recorded image output has no canvas preview`);
    assert(preview.digest === eligible[0], `${label}/${node.id}: thumbnail does not use the first eligible output image`);
    assert(preview.url === `/api/artifacts/${preview.digest}`, `${label}/${node.id}: thumbnail URL is not its confined artifact endpoint`);
    assert(preview.count === eligible.length, `${label}/${node.id}: thumbnail count does not match distinct available image outputs`);
    assert(preview.label.length > 0, `${label}/${node.id}: thumbnail has no accessible label`);
  }
}
function checkLayout(graph: CanvasGraph, label: string) {
  const first = layoutGraph(graph);
  const second = layoutGraph(graph);
  assert(JSON.stringify(first) === JSON.stringify(second), `${label}: layout changes for identical input`);
  assert(Number.isFinite(first.width) && Number.isFinite(first.height), `${label}: layout has nonfinite bounds`);
  for (const node of first.nodes) {
    assert([node.x, node.y, node.width, node.height].every(Number.isFinite), `${label}/${node.source_id}: nonfinite node geometry`);
    assert(node.width > 0 && node.height > 0, `${label}/${node.source_id}: empty node geometry`);
    for (const side of ["input", "output"] as const) {
      const ports = side === "input" ? node.ports?.inputs ?? [] : node.ports?.outputs ?? [];
      const anchors = ports.map((port) => canvasPortAnchor(node, side, port.name, port.kind));
      assert(anchors.every((point) => point && Number.isFinite(point.x) && Number.isFinite(point.y)), `${label}/${node.source_id}: a named port has no finite anchor`);
      assert(new Set(anchors.map((point) => `${point?.x},${point?.y}`)).size === anchors.length, `${label}/${node.source_id}: named ${side} ports share one anchor`);
    }
  }
  for (let left = 0; left < first.nodes.length; left++) {
    const a = first.nodes[left];
    for (const b of first.nodes.slice(left + 1)) {
      const overlap = a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
      assert(!overlap, `${label}: nodes overlap: ${a.source_id}, ${b.source_id}`);
    }
  }
  for (const edge of first.edges) {
    assert(edge.points.length >= 2 && edge.points.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y)), `${label}: edge ${edge.id} has invalid geometry`);
    if (edge.kind !== "data") continue;
    const source = first.nodes.find((node) => node.id === edge.source);
    const target = first.nodes.find((node) => node.id === edge.target);
    assert(source && target && edge.source_port && edge.target_port, `${label}: data edge has no named endpoints`);
    const sourcePort = source.ports?.outputs.find((port) => port.name === edge.source_port && (edge.source_kind === "fact" ? port.kind === "fact" : port.kind !== "fact"));
    assert(sourcePort, `${label}: data edge has no matching source socket`);
    const from = canvasPortAnchor(source, "output", edge.source_port, sourcePort.kind);
    const to = canvasPortAnchor(target, "input", edge.target_port);
    assert(from && to && JSON.stringify(edge.points[0]) === JSON.stringify(from) && JSON.stringify(edge.points.at(-1)) === JSON.stringify(to), `${label}: data edge ${edge.id} does not meet its named socket anchors`);
  }
  const frames = new Map((first.frames ?? []).map((frame) => [frame.id, frame]));
  for (const frame of frames.values()) {
    assert([frame.x, frame.y, frame.width, frame.height].every(Number.isFinite) && frame.width > 0 && frame.height > 0, `${label}/${frame.id}: invalid inline group geometry`);
    const contains = (child: { x: number; y: number; width: number; height: number }) => child.x >= frame.x && child.y >= frame.y && child.x + child.width <= frame.x + frame.width && child.y + child.height <= frame.y + frame.height;
    for (const id of frame.nodes) {
      const child = first.nodes.find((node) => node.id === id);
      assert(child && contains(child), `${label}/${frame.id}: inline group does not contain its direct child ${id}`);
    }
    for (const child of frames.values()) if (child.parent === frame.id) assert(contains(child), `${label}/${frame.id}: nested inline group escapes its parent`);
  }
}

function checkScopes(view: ViewerView, expected: CaseExpectation, label: string) {
  const scopes = view.scopes ?? [];
  const byId = new Map(scopes.map((scope) => [scope.id, scope]));
  const byPath = new Map(scopes.map((scope) => [scope.path, scope]));
  const source = view.kind === "fx-graph-v1" ? view.instances : view.nodes;
  const byNode = new Map(source.map((node) => [node.id, node]));
  assert(byId.size === scopes.length, `${label}: distinct scope IDs collapsed`);
  const owned = new Set<string>();
  for (const scope of scopes) {
    assert(scope.parent === null || byId.has(scope.parent), `${label}/${scope.path}: missing scope parent`);
    const ancestors = new Set([scope.id]);
    let parent = scope.parent;
    while (parent !== null) {
      assert(!ancestors.has(parent), `${label}/${scope.path}: cyclic scope ancestry`);
      ancestors.add(parent);
      parent = byId.get(parent)!.parent;
    }
    for (const id of scope.nodes) {
      assert(byNode.has(id), `${label}/${scope.path}: scope claims an absent leaf`);
      assert(!owned.has(id), `${label}/${id}: leaf has more than one direct scope`);
      owned.add(id);
    }
  }
  const enclosingWorkflow = (parent: string | null): string | null => {
    while (parent !== null) {
      const scope = byId.get(parent)!;
      if (scope.kind === "workflow") return scope.id;
      parent = scope.parent;
    }
    return null;
  };
  for (const required of expected.requires_scopes ?? []) {
    const scope = byPath.get(required.path);
    assert(scope, `${label}: missing required scope ${required.path}`);
    assert(scope.kind === required.kind, `${label}/${required.path}: wrong scope kind`);
    assert((scope.parent === null ? null : byId.get(scope.parent)?.path) === required.parent, `${label}/${required.path}: wrong parent scope`);
    if (required.source !== undefined) assert(scope.source === required.source, `${label}/${required.path}: wrong imported source`);
    for (const field of ["inputs", "outputs", "nodes"] as const) {
      if (required[field] === undefined) continue;
      const actual = field === "nodes" ? scope.nodes : field === "inputs" ? Object.keys(scope.ports.inputs) : scope.ports.outputs;
      assert(JSON.stringify([...actual].sort()) === JSON.stringify([...required[field]!].sort()), `${label}/${required.path}: scope ${field} changed`);
    }
    if (scope.kind !== "workflow") continue;
    const outer = canvasGraph(view, enclosingWorkflow(scope.parent));
    const card = outer.nodes.find((node) => node.kind === "workflow" && node.scope_id === scope.id);
    assert(card, `${label}/${required.path}: imported workflow has no outer card`);
    assert(JSON.stringify(card.ports?.inputs.map((port) => port.name).sort()) === JSON.stringify(Object.keys(scope.ports.inputs).sort()), `${label}/${required.path}: boundary input aliases changed`);
    assert(JSON.stringify(card.ports?.outputs.map((port) => port.name).sort()) === JSON.stringify([...scope.ports.outputs].sort()), `${label}/${required.path}: boundary output aliases changed`);
    assert(card.ports?.outputs.every((port) => port.kind === "value" && port.type === "unknown"), `${label}/${required.path}: untyped workflow outputs acquired invented artifact types`);
    if (required.state !== undefined) assert(card.state === required.state, `${label}/${required.path}: enclosing card hid descendant state ${required.state}`);
    if (required.state === "failed") assert((card.failure_count ?? 0) > 0 && (card.errors?.length ?? 0) > 0, `${label}/${required.path}: failed imported card lost descendant errors`);
    const inside = canvasGraph(view, scope.id);
    count(required.visible_count, inside.nodes.length, `${label}/${required.path} visible nodes`);
    for (const id of scope.nodes) {
      assert(!outer.nodes.some((node) => node.source_id === id), `${label}/${required.path}: child leaked into outer canvas`);
      assert(inside.nodes.some((node) => node.source_id === id), `${label}/${required.path}: direct child is missing inside its workflow`);
    }
  }
  for (const required of expected.requires_interfaces ?? []) {
    const target = required.target_kind === "node" ? byNode.get(required.target) : byPath.get(required.target);
    assert(target, `${label}: missing interface target ${required.target}`);
    const bindings = required.target_kind === "node" ? (target as RunNode).interface_bindings : required.target_kind === "scope_input" ? (target as typeof scopes[number]).input_bindings : (target as typeof scopes[number]).output_bindings;
    const sourceId = required.source_kind === "scope_input" || required.source_kind === "scope_output" ? byPath.get(required.source)?.id : required.source;
    assert(sourceId && bindings?.some((binding) => binding.source === sourceId && binding.source_port === required.source_port && binding.source_kind === required.source_kind && binding.target_port === required.target_port), `${label}: exact workflow interface alias missing: ${required.source}.${required.source_port} -> ${required.target}.${required.target_port}`);
    const targetScope = required.target_kind === "node" ? undefined : byPath.get(required.target)!;
    const owner = required.target_kind === "node" ? scopes.find((scope) => scope.nodes.includes(required.target))?.id ?? null : required.target_kind === "scope_input" ? targetScope!.parent : targetScope!.id;
    const current = enclosingWorkflow(owner);
    const graph = canvasGraph(view, current);
    const targetId = required.target_kind === "node" ? `instance:${required.target}` : required.target_kind === "scope_input" ? targetScope!.id : `boundary:output:${targetScope!.id}`;
    const from = required.source_kind === "scope_input" ? `boundary:input:${sourceId}` : required.source_kind === "scope_output" ? sourceId : `instance:${sourceId}`;
    assert(graph.edges.some((edge) => edge.kind === "data" && edge.source === from && edge.target === targetId && edge.source_port === required.source_port && edge.target_port === required.target_port && edge.source_kind === (required.source_kind === "fact" ? "fact" : "output")), `${label}: scoped canvas lost named alias wire ${required.source}.${required.source_port} -> ${required.target}.${required.target_port}`);
  }
  const root = canvasGraph(view, null);
  count(expected.root_node_count, root.nodes.length, `${label} root scope nodes`);
  for (const current of [null, ...scopes.filter((scope) => scope.kind === "workflow").map((scope) => scope.id)]) {
    const graph = canvasGraph(view, current);
    checkLayout(graph, `${label}/${current ?? "root"}`);
    for (const group of scopes.filter((scope) => scope.kind === "group" && enclosingWorkflow(scope.parent) === current)) {
      const frame = graph.frames?.find((item) => item.id === group.id);
      assert(frame, `${label}/${group.path}: inline group lost its frame`);
      for (const id of group.nodes) {
        const node = graph.nodes.find((item) => item.source_id === id);
        assert(node && frame.nodes.includes(node.id), `${label}/${group.path}: inline child left its frame`);
      }
    }
  }
  if (expected.legacy) {
    assert(scopes.length === 0, `${label}: legacy case retained scope metadata`);
    assert(root.nodes.every((node) => node.kind !== "workflow"), `${label}: old paths gained invented workflow cards`);
    count(source.length + (view.kind === "fx-graph-v1" ? view.pending.length : 0), root.nodes.length, `${label} legacy flat fallback`);
  }
}

function checkPorts(view: ViewerView, graph: CanvasGraph, expected: CaseExpectation, label: string) {
  const source = view.kind === "fx-graph-v1" ? view.instances : view.nodes;
  const bySource = new Map(source.map((node) => [node.id, node]));
  const byCanvas = new Map(graph.nodes.map((node) => [node.id, node]));
  for (const node of source) {
    const declared = view.kind === "fx-graph-v1" ? view.types?.[node.uses ?? ""]?.ports : (node as RunNode).ports;
    if (!declared) continue;
    const projected = graph.nodes.find((item) => item.source_id === node.id);
    assert(projected?.ports, `${label}/${node.id}: declared ports disappeared from the canvas`);
    for (const side of ["inputs", "outputs"] as const) {
      const artifacts = projected.ports[side].filter((port) => port.kind === "artifact").map((port) => [port.name, port.type]).sort();
      assert(JSON.stringify(artifacts) === JSON.stringify(Object.entries(declared[side]).sort()), `${label}/${node.id}: canvas ${side} changed declared artifact sockets`);
    }
    const settings = [...projected.ports.settings.map((setting) => setting.name), ...projected.ports.inputs.filter((port) => port.kind === "parameter").map((port) => port.name)].sort();
    assert(JSON.stringify(settings) === JSON.stringify(Object.keys(declared.params).sort()), `${label}/${node.id}: canvas duplicated or omitted declared parameters`);
  }
  for (const required of expected.requires_ports ?? []) {
    const node = bySource.get(required.id);
    assert(node, `${label}: missing port metadata node ${required.id}`);
    const ports = view.kind === "fx-graph-v1" ? view.types?.[node.uses ?? ""]?.ports : (node as RunNode).ports;
    assert(ports, `${label}/${required.id}: missing declared port metadata`);
    for (const side of ["inputs", "outputs"] as const) {
      if (!required[side]) continue;
      assert(JSON.stringify(Object.entries(ports[side]).sort()) === JSON.stringify(Object.entries(required[side]!).sort()), `${label}/${required.id}: declared ${side} ports changed`);
    }
    if (required.params) assert(JSON.stringify(Object.keys(ports.params).sort()) === JSON.stringify([...required.params].sort()), `${label}/${required.id}: declared parameter names changed`);
  }
  for (const required of expected.requires_bindings ?? []) {
    const target = bySource.get(required.target);
    assert(target?.bindings?.some((binding) => binding.source === required.source && binding.source_port === required.source_port && binding.target_port === required.target_port && binding.source_kind === required.source_kind), `${label}: missing exact binding ${required.source}.${required.source_port} -> ${required.target}.${required.target_port}`);
  }
  for (const setting of expected.literal_settings ?? []) {
    const node = graph.nodes.find((item) => item.source_id === setting.id);
    assert(node?.ports, `${label}: literal setting has no declared node ports`);
    assert(node.ports.settings.some((item) => item.name === setting.name), `${label}: literal ${setting.id}.${setting.name} did not remain a setting`);
    assert(!node.ports.inputs.some((port) => port.name === setting.name), `${label}: literal ${setting.id}.${setting.name} gained a data socket`);
  }
  assert(new Set(graph.edges.map((edge) => edge.id)).size === graph.edges.length, `${label}: canvas edge IDs collapsed distinct connections`);
  for (const target of source) {
    for (const binding of target.bindings ?? []) {
      const matches = graph.edges.filter((edge) => edge.kind === "data" && byCanvas.get(edge.source)?.source_id === binding.source && byCanvas.get(edge.target)?.source_id === target.id && edge.source_port === binding.source_port && edge.target_port === binding.target_port && edge.source_kind === binding.source_kind);
      assert(matches.length === 1, `${label}: expected exactly one data edge for ${binding.source}.${binding.source_port} -> ${target.id}.${binding.target_port}`);
    }
  }
  for (const edge of graph.edges) {
    if (edge.kind !== "data") {
      assert(edge.source_port === undefined && edge.target_port === undefined && edge.source_kind === undefined, `${label}: a control or unknown dependency claims named data sockets`);
      continue;
    }
    const from = byCanvas.get(edge.source);
    const to = byCanvas.get(edge.target);
    assert(from && to, `${label}: a named edge has an absent endpoint`);
    assert(from.ports?.outputs.some((port) => port.name === edge.source_port && port.kind === (edge.source_kind === "fact" ? "fact" : "artifact")), `${label}: a named edge invents its source port`);
    assert(to.ports?.inputs.some((port) => port.name === edge.target_port), `${label}: a named edge invents its target port`);
    assert(bySource.get(to.source_id)?.bindings?.some((binding) => binding.source === from.source_id && binding.source_port === edge.source_port && binding.target_port === edge.target_port && binding.source_kind === edge.source_kind), `${label}: a data edge was guessed without a recorded binding`);
  }
  if (expected.legacy) {
    assert(source.every((node) => node.bindings === undefined), `${label}: legacy fixture still carries binding metadata`);
    assert(graph.edges.every((edge) => edge.kind !== "data"), `${label}: an old record gained invented named data wires`);
    assert(graph.edges.some((edge) => edge.kind === "dependency"), `${label}: legacy dependencies disappeared`);
  }
}
async function checkCase(root: string, section: "plans" | "views", expected: CaseExpectation) {
  const label = `${section}/${expected.file}`;
  const view = parseViewerView(readJson(join(root, section), expected.file));
  assert(view.kind === (section === "plans" ? "fx-graph-v1" : "fx-viewer-run-v1"), `${label}: wrong document kind`);
  const original = JSON.stringify(view);
  const graph = flatCanvasGraph(view);
  const sourceNodes = view.kind === "fx-graph-v1" ? view.instances : view.nodes;
  const pendingNodes = view.kind === "fx-graph-v1" ? view.pending.length : 0;
  count(sourceNodes.length + pendingNodes, graph.nodes.length, `${label} preserved source nodes`);
  for (const node of sourceNodes) assert(graph.nodes.some((item) => !item.pending && item.source_id === node.id), `${label}: missing source instance ${node.id}`);
  assert(new Set(graph.nodes.map((node) => node.id)).size === graph.nodes.length, `${label}: duplicate canvas IDs`);
  assert(new Set(graph.nodes.map((node) => node.source_id)).size === graph.nodes.length, `${label}: repeated instance identity collapsed`);
  const ids = new Set(graph.nodes.map((node) => node.id));
  for (const edge of graph.edges) assert(ids.has(edge.source) && ids.has(edge.target), `${label}: edge references an absent canvas node`);
  count(expected.node_count, graph.nodes.length, `${label} nodes`);
  count(expected.edge_count, graph.edges.length, `${label} edges`);
  count(expected.pending_count, graph.nodes.filter((node) => node.pending).length, `${label} pending repeats`);
  count(expected.preview_count, graph.nodes.filter((node) => node.preview).length, `${label} image previews`);
  for (const id of expected.requires_nodes ?? []) assert(graph.nodes.some((node) => node.source_id === id), `${label}: missing required instance or pending repeat ${id}`);
  for (const state of expected.required_states ?? []) assert(graph.nodes.some((node) => node.state === state), `${label}: missing required node state ${state}`);
  const byId = new Map(graph.nodes.map((node) => [node.id, node.source_id]));
  for (const required of expected.requires_edges ?? []) {
    assert(graph.edges.some((edge) => byId.get(edge.source) === required.source && byId.get(edge.target) === required.target && (required.kind === undefined || edge.kind === required.kind) && (required.kinds ?? []).every((kind) => edge.kinds.includes(kind))), `${label}: missing required edge ${required.source} -> ${required.target}`);
  }
  if (view.kind === "fx-viewer-run-v1") {
    if (expected.state !== undefined) assert(view.state === expected.state, `${label}: expected run state ${expected.state}, received ${view.state}`);
    count(expected.cache_hits, view.nodes.filter((node) => node.cache === "hit").length, `${label} cache hits`);
  } else {
    assert(expected.state === undefined && expected.cache_hits === undefined, `${label}: static plans cannot assert run status or cache hits`);
    count(view.pending.length, graph.nodes.filter((node) => node.pending).length, `${label} preserved pending repeats`);
  }
  checkImages(view, graph, label);
  checkPorts(view, graph, expected, label);
  checkLayout(graph, label);
  checkScopes(view, expected, label);
  const controller = new ViewerController(async () => view);
  await controller.refresh();
  const visited = new Set<string>();
  const navigate = () => {
    const current = controller.getSnapshot();
    for (const node of current.graph.nodes) {
      controller.select(node.id);
      assert(controller.getSnapshot().selected === node.id, `${label}: visible node ${node.id} cannot be selected distinctly`);
    }
    for (const node of current.graph.nodes.filter((item) => item.kind === "workflow")) {
      assert(node.scope_id && !visited.has(node.scope_id), `${label}: imported scope cannot be entered distinctly`);
      visited.add(node.scope_id);
      controller.openScope(node.scope_id);
      const inside = controller.getSnapshot();
      assert(inside.scope === node.scope_id && inside.breadcrumbs.at(-1)?.id === node.scope_id && inside.breadcrumbs[0]?.id === null, `${label}: entering an import lost its breadcrumb identity`);
      navigate();
      controller.back();
      assert(controller.getSnapshot().scope === current.scope, `${label}: Back did not return to the enclosing workflow`);
    }
  };
  navigate();
  count((view.scopes ?? []).filter((scope) => scope.kind === "workflow").length, visited.size, `${label} navigable imported workflows`);
  controller.dispose();
  assert(JSON.stringify(view) === original, `${label}: browser projection, layout, or selection mutated the source`);
  console.log(`PASS ${label}: ${graph.nodes.length} nodes, ${graph.edges.length} edges, ${graph.nodes.filter((node) => node.pending).length} pending, ${graph.nodes.filter((node) => node.preview).length} image previews`);
}

async function main() {
  assert(process.argv.length === 3, "usage: bun --no-env-file tools/check_viewer_fixtures.ts <generated-fixture-directory>");
  const root = resolve(process.argv[2]);
  checkDirectory(root);
  checkDirectory(join(root, "plans"));
  checkDirectory(join(root, "views"));
  const expected = readExpectations(root);
  for (const item of expected.plans) await checkCase(root, "plans", item);
  for (const item of expected.views) await checkCase(root, "views", item);
  console.log(`viewer fixtures: ${expected.plans.length} plans and ${expected.views.length} recorded run views passed`);
  console.log("Scope: verifies declared ports, exact recorded bindings, control and legacy dependency fallback, socket geometry, and current read contracts; browser rendering and complete execution history still need separate review.");
}

try {
  await main();
} catch (error) {
  console.error(`check_viewer_fixtures: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
