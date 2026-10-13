import { createElement, Play } from "lucide";
import { displayPortName } from "./ports";
import { canvasNodeGeometry, canvasPreviewReserved, PORT_ROW, PORT_TOP, type PositionedNode } from "./layout";
import { stateIcon, stateIconParts } from "./state-icon";
import type { CanvasStep } from "./graph";

const SVG = "http://www.w3.org/2000/svg";

export function element<K extends keyof SVGElementTagNameMap>(tag: K, attributes: Record<string, string | number> = {}, text?: string): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, String(value));
  if (text !== undefined) node.textContent = text;
  return node;
}
/** How long a running card has run, beside its spinner. */
export function runningLabel(started: number, now: number) { return elapsedLabel(now - started); }
/** A time that counts in whole seconds: "8s", "1m 04s", "2h 05m". */
export function elapsedLabel(ms: number) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  const [hours, minutes, rest] = [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60];
  const two = (value: number) => String(value).padStart(2, "0");
  return hours ? `${hours}h ${two(minutes)}m` : minutes ? `${minutes}m ${two(rest)}s` : `${rest}s`;
}

/**
 * A card's status line: a running card counts up from its start less the time it waited for
 * route slots; a waiting one names its route and counts up from when its wait began. `from` is
 * absent when the record cannot date it; the line is then `text` alone.
 */
export function cardStatus(node: Pick<CanvasStep, "state" | "started_at" | "waited_ms" | "waiting_for" | "waiting_since">, routeLimit = 18): { text: string; from?: number } {
  const time = (value: string | undefined) => value ? Date.parse(value) : NaN;
  if (node.state === "running") {
    const started = time(node.started_at);
    return Number.isNaN(started) ? { text: node.state } : { text: "", from: started + (node.waited_ms ?? 0) };
  }
  if (node.state === "waiting") {
    const text = node.waiting_for ? `waiting for ${short(node.waiting_for, routeLimit)}` : "waiting";
    const since = time(node.waiting_since);
    return Number.isNaN(since) ? { text } : { text: `${text} · `, from: since };
  }
  return { text: node.state };
}

/** A state's glyph as a nested SVG square of `size` at (x, y). */
export function stateIconElement(state: string, x: number, y: number, size: number) {
  const icon = stateIcon(state);
  const svg = element("svg", { x, y, width: size, height: size, viewBox: "0 0 16 16", class: "fx-state-icon", "data-icon": icon, "aria-hidden": "true" });
  const glyph = element("g", { class: "fx-state-glyph" });
  for (const part of stateIconParts(icon)) glyph.append(element(part.tag, part.attributes));
  svg.append(glyph);
  return svg;
}

export function short(text: string, limit: number) { return [...text].length > limit ? [...text].slice(0, limit - 1).join("") + "…" : text; }

/**
 * One node card at its absolute position. The caller wires selection and the open-workflow
 * control; `signal` removes the card's own listeners when the canvas is redrawn.
 */
export function nodeCard(node: PositionedNode, canvasId: string, index: number, signal: AbortSignal, onOpen: (scope: string) => void) {
  const preview = node.preview;
  const portsLabel = node.ports ? `, inputs: ${node.ports.inputs.map((port) => `${displayPortName(port.name)} (${port.type}, ${port.kind})`).join(", ") || "none"}, outputs: ${node.ports.outputs.map((port) => `${displayPortName(port.name)} (${port.type}, ${port.kind})`).join(", ") || "none"}` : node.pending ? "" : ", port metadata not recorded";
  const scopeLabel = node.kind === "workflow" ? `, imported workflow, ${node.child_count ?? 0} steps, ${node.failure_count ?? 0} failed` : "";
  const state = node.state === "waiting" && node.waiting_for ? `waiting for ${node.waiting_for}` : node.state;
  const count = preview ? `${preview.count} ${preview.kind} ${preview.count === 1 ? "output" : "outputs"}` : "";
  const label = `${node.title}, ${state}${scopeLabel}${portsLabel}${preview ? `, ${preview.label}, ${count}` : ""}`;
  const group = element("g", { transform: `translate(${node.x},${node.y})`, class: `fx-canvas-node${node.kind === "workflow" ? " fx-canvas-workflow" : node.kind === "boundary" ? " fx-canvas-boundary" : ""}`, role: node.kind === "workflow" ? "group" : "button", tabindex: 0, "data-node-id": node.id, "data-state": node.state, "aria-label": label, ...(node.kind === "workflow" ? { "aria-current": "false" } : { "aria-pressed": "false" }) });
  group.append(element("rect", { width: node.width, height: node.height, rx: 11, class: node.pending ? "fx-canvas-node-box fx-canvas-pending" : "fx-canvas-node-box" }));
  group.append(element("text", { x: 16, y: 27, class: "fx-canvas-node-title" }, short(node.title, node.ports ? 42 : 28)));
  group.append(element("text", { x: 16, y: 47, class: "fx-canvas-node-type" }, short(node.kind === "workflow" ? node.path ?? node.subtitle : node.subtitle, node.ports ? 51 : 34)));
  if (node.ports) {
    group.append(element("line", { x1: 0, y1: 59, x2: node.width, y2: 59, class: "fx-canvas-node-divider" }));
    group.append(element("text", { x: 16, y: 76, class: "fx-canvas-port-heading" }, "INPUTS"));
    group.append(element("text", { x: node.width - 16, y: 76, "text-anchor": "end", class: "fx-canvas-port-heading" }, "OUTPUTS"));
    for (const direction of ["input", "output"] as const) {
      const ports = direction === "input" ? node.ports.inputs : node.ports.outputs;
      const x = direction === "input" ? 0 : node.width;
      const textX = direction === "input" ? 16 : node.width - 16;
      for (const [row, port] of ports.entries()) {
        const y = PORT_TOP + row * PORT_ROW;
        const socket = element("g", { class: `fx-canvas-port fx-canvas-port-${port.kind}`, "data-port-name": port.name, "data-port-direction": direction, "data-port-kind": port.kind });
        socket.append(port.kind === "artifact" ? element("circle", { cx: x, cy: y, r: 4.5 })
          : port.kind === "fact" ? element("path", { d: `M${x} ${y - 5}l5 5l-5 5l-5 -5z` })
          : element("rect", { x: x - 4, y: y - 4, width: 8, height: 8, rx: 1 }));
        socket.append(element("text", { x: textX, y: y + 3, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-name" }, short(displayPortName(port.name), 22)));
        socket.append(element("text", { x: textX, y: y + 16, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-type" }, short(`${port.kind === "parameter" ? "param · " : ""}${port.type}`, 25)));
        group.append(socket);
      }
      if (!ports.length) group.append(element("text", { x: textX, y: PORT_TOP + 3, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-type" }, "None"));
    }
    if (node.ports.settings.length) {
      const settings = element("text", { x: 16, y: canvasNodeGeometry(node).settings_y + 10, class: "fx-canvas-settings" }, short(`Settings · ${node.ports.settings.map((setting) => setting.name).join(", ")}`, 52));
      group.append(settings);
    }
  } else if (!node.pending && !preview) {
    group.append(element("text", { x: 16, y: 64, class: "fx-canvas-port-type" }, "Port metadata not recorded"));
  }
  if (preview) {
    const frame = { x: 16, y: canvasNodeGeometry(node).preview_y, width: node.width - 32, height: 140 };
    const clipId = `${canvasId}-preview-${index}`;
    const clip = element("clipPath", { id: clipId });
    clip.append(element("rect", { ...frame, rx: 6 }));
    group.append(clip);
    const backdrop = element("rect", { ...frame, rx: 6, fill: `url(#${canvasId}-checker)` });
    const unavailable = preview.kind === "video" ? "Video unavailable" : "Image unavailable";
    const image = element("image", { ...frame, preserveAspectRatio: "xMidYMid meet", "clip-path": `url(#${clipId})`, role: "img", "aria-label": preview.label, class: "fx-canvas-node-image" });
    const fallback = element("text", { x: node.width / 2, y: frame.y + frame.height / 2, "text-anchor": "middle", "dominant-baseline": "middle", class: "fx-canvas-image-error", visibility: "hidden" }, unavailable);
    image.addEventListener("error", () => {
      image.setAttribute("visibility", "hidden");
      fallback.setAttribute("visibility", "visible");
      group.setAttribute("aria-label", `${node.title}, ${state}${portsLabel}, ${unavailable}: ${preview.label}`);
    }, { signal });
    const border = element("rect", { ...frame, rx: 6, class: "fx-canvas-image-border" });
    if (preview.kind === "image") {
      image.setAttribute("href", preview.url);
      group.append(backdrop, image, border, fallback);
    } else {
      // A video shows its first frame, which the canvas sets once the card is in view, and a play
      // control; the canvas plays it over the frame (video.ts). The backdrop takes the pointer, so
      // resting anywhere on the frame plays it muted.
      const video = element("g", { class: "fx-canvas-video", "data-digest": preview.digest });
      const play = element("g", { class: "fx-canvas-play", role: "button", tabindex: 0, "aria-label": `Play ${preview.label}` });
      const center = { x: node.width / 2, y: frame.y + frame.height / 2 };
      play.append(element("circle", { cx: center.x, cy: center.y, r: 18 }),
        createElement(Play, { x: center.x - 8, y: center.y - 8, width: 16, height: 16, "stroke-width": 2, "aria-hidden": "true" }));
      video.append(backdrop, image, border, fallback, play);
      group.append(video);
    }
    group.append(element("text", { x: node.width - 16, y: node.height - 22, "text-anchor": "end", class: "fx-canvas-image-count" }, count));
  } else if (canvasPreviewReserved(node)) {
    const frame = { x: 16, y: canvasNodeGeometry(node).preview_y, width: node.width - 32, height: 140 };
    // Only a started step shimmers: the declared file may be on its way. Waiting and settled steps keep the empty frame.
    if (node.state === "running") {
      const clipId = `${canvasId}-shimmer-${index}`;
      const clip = element("clipPath", { id: clipId });
      clip.append(element("rect", { ...frame, rx: 6 }));
      const shimmer = element("g", { "clip-path": `url(#${clipId})`, "aria-hidden": "true" });
      shimmer.append(
        element("rect", { ...frame, class: "fx-canvas-shimmer-base" }),
        element("rect", { ...frame, x: frame.x - frame.width, fill: `url(#${canvasId}-shimmer)`, class: "fx-canvas-shimmer-band" }),
      );
      group.append(clip, shimmer);
    }
    group.append(element("rect", { ...frame, rx: 6, class: "fx-canvas-image-border" }));
  }
  group.append(stateIconElement(node.state, 15, node.height - 32, 12));
  // A dated running or waiting card carries what it counts from; the canvas recounts it every second.
  const status = cardStatus(node, node.ports ? 18 : 12);
  group.append(status.from === undefined
    ? element("text", { x: 32, y: node.height - 22, class: "fx-canvas-node-state" }, status.text)
    : element("text", { x: 32, y: node.height - 22, class: "fx-canvas-node-state", "data-started": status.from, "data-prefix": status.text }, status.text + runningLabel(status.from, Date.now())));
  if (node.kind === "workflow" && node.scope_id) {
    group.append(element("text", { x: 16, y: node.height - 48, class: (node.failure_count ?? 0) > 0 ? "fx-canvas-workflow-failure" : "fx-canvas-workflow-count" }, `${node.child_count ?? 0} steps${node.failure_count ? ` · ${node.failure_count} failed` : ""}`));
    const open = element("g", { class: "fx-canvas-open-workflow", role: "button", tabindex: 0, "aria-label": `Open workflow ${node.title}` });
    open.append(element("rect", { x: node.width - 144, y: node.height - 37, width: 128, height: 24, rx: 5 }));
    open.append(element("text", { x: node.width - 80, y: node.height - 21, "text-anchor": "middle" }, "Open workflow →"));
    const enter = () => onOpen(node.scope_id!);
    open.addEventListener("click", (event) => { event.stopPropagation(); enter(); }, { signal });
    open.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); event.stopPropagation(); enter(); }
    }, { signal });
    group.addEventListener("dblclick", enter, { signal });
    group.append(open);
  }
  return group;
}
