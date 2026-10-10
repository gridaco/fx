import { displayPortName } from "./ports";
import { canvasNodeGeometry, canvasPreviewReserved, PORT_ROW, PORT_TOP, type PositionedNode } from "./layout";

const SVG = "http://www.w3.org/2000/svg";

export function element<K extends keyof SVGElementTagNameMap>(tag: K, attributes: Record<string, string | number> = {}, text?: string): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, String(value));
  if (text !== undefined) node.textContent = text;
  return node;
}
/** A running card's state and how long it has run: "running · 8s", "· 1m 04s", "· 2h 05m". */
export function runningLabel(started: number, now: number) {
  const seconds = Math.max(0, Math.floor((now - started) / 1000));
  const [hours, minutes, rest] = [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60];
  const two = (value: number) => String(value).padStart(2, "0");
  return `running · ${hours ? `${hours}h ${two(minutes)}m` : minutes ? `${minutes}m ${two(rest)}s` : `${rest}s`}`;
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
  const label = `${node.title}, ${node.state}${scopeLabel}${portsLabel}${preview ? `, ${preview.label}, ${preview.count} image ${preview.count === 1 ? "output" : "outputs"}` : ""}`;
  const group = element("g", { transform: `translate(${node.x},${node.y})`, class: `fx-canvas-node${node.kind === "workflow" ? " fx-canvas-workflow" : node.kind === "boundary" ? " fx-canvas-boundary" : ""}`, role: node.kind === "workflow" ? "group" : "button", tabindex: 0, "data-node-id": node.id, "data-state": node.state, "aria-label": label, ...(node.kind === "workflow" ? { "aria-current": "false" } : { "aria-pressed": "false" }) });
  group.append(element("title", {}, `${node.title}\n${node.subtitle}\n${node.source_id}`));
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
        socket.append(element("title", {}, `${direction}: ${displayPortName(port.name)} · ${port.type} · ${port.kind}`));
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
      settings.append(element("title", {}, node.ports.settings.map((setting) => `${setting.name}: ${setting.type}`).join("\n")));
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
    group.append(clip, element("rect", { ...frame, rx: 6, fill: `url(#${canvasId}-checker)` }));
    const image = element("image", { ...frame, preserveAspectRatio: "xMidYMid meet", "clip-path": `url(#${clipId})`, role: "img", "aria-label": preview.label, class: "fx-canvas-node-image" });
    image.append(element("title", {}, preview.label));
    const fallback = element("text", { x: node.width / 2, y: frame.y + frame.height / 2, "text-anchor": "middle", "dominant-baseline": "middle", class: "fx-canvas-image-error", visibility: "hidden" }, "Image unavailable");
    image.addEventListener("error", () => {
      image.setAttribute("visibility", "hidden");
      fallback.setAttribute("visibility", "visible");
      group.setAttribute("aria-label", `${node.title}, ${node.state}${portsLabel}, Image unavailable: ${preview.label}`);
    }, { signal });
    image.setAttribute("href", preview.url);
    group.append(image, element("rect", { ...frame, rx: 6, class: "fx-canvas-image-border" }), fallback);
    group.append(element("text", { x: node.width - 16, y: node.height - 22, "text-anchor": "end", class: "fx-canvas-image-count" }, `${preview.count} image ${preview.count === 1 ? "output" : "outputs"}`));
  } else if (canvasPreviewReserved(node)) {
    group.append(element("rect", { x: 16, y: canvasNodeGeometry(node).preview_y, width: node.width - 32, height: 140, rx: 6, class: "fx-canvas-image-border" }));
  }
  if (node.state === "running") group.append(element("circle", { cx: 21, cy: node.height - 26, r: 3.5, class: "fx-canvas-running-indicator", "aria-hidden": "true" }));
  // A dated running card carries its start; the canvas recounts it every second.
  const started = node.state === "running" && node.started_at ? Date.parse(node.started_at) : NaN;
  group.append(Number.isNaN(started)
    ? element("text", { x: node.state === "running" ? 32 : 16, y: node.height - 22, class: "fx-canvas-node-state" }, node.state)
    : element("text", { x: 32, y: node.height - 22, class: "fx-canvas-node-state", "data-started": started }, runningLabel(started, Date.now())));
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
