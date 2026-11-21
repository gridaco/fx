import { valuePreviewLines, valueStateLabel } from "./value-views";
import { displayPortName } from "./ports";
import { canvasNodeGeometry, canvasPreviewReserved, reviewFooter, PORT_ROW, PORT_TOP, type PositionedNode } from "./layout";
import { stateIcon, stateIconParts } from "./state-icon";
import type { CanvasStep } from "./graph";
import { skipPreviewLines } from "./skip";
import { countdownLabel } from "./waiting";
import { Box, Braces, CircleSlash, createElement, RotateCw, VideoOff } from "lucide";

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
export function cardStatus(node: Pick<CanvasStep, "state" | "review_label" | "review_action" | "started_at" | "waited_ms" | "waiting_for" | "waiting_since">, routeLimit = 18): { text: string; from?: number } {
  const time = (value: string | undefined) => value ? Date.parse(value) : NaN;
  if (node.review_action === "review") return { text: "Awaiting answer" };
  if (node.state === "awaiting") {
    if (node.review_action === "view_answer") return { text: "Answer recorded" };
    if (node.review_action === "view_outcome") return { text: "Expired" };
    return { text: "Awaiting answer" };
  }
  if (node.state === "running") {
    const started = time(node.started_at);
    return Number.isNaN(started) ? { text: node.state } : { text: "", from: started + (node.waited_ms ?? 0) };
  }
  if (node.state === "waiting") {
    const text = node.waiting_for ? `waiting for ${short(node.waiting_for, routeLimit)}` : "waiting";
    const since = time(node.waiting_since);
    return Number.isNaN(since) ? { text } : { text: `${text} · `, from: since };
  }
  return { text: node.review_label ? `${node.state} · ${node.review_label}` : node.state };
}

/**
 * Filled play and pause glyphs on a 24-unit square, in the current colour. Unlike the rest of the
 * viewer's chrome they are not Lucide's: its outlined ones read poorly on a video.
 */
export function mediaGlyph(kind: "play" | "pause", attributes: Record<string, string | number> = {}) {
  const svg = element("svg", { viewBox: "0 0 24 24", fill: "currentColor", "aria-hidden": "true", ...attributes });
  if (kind === "play") svg.append(element("path", { d: "M7 4.5v15a1 1 0 0 0 1.53.85l12-7.5a1 1 0 0 0 0-1.7l-12-7.5A1 1 0 0 0 7 4.5z" }));
  else for (const x of [6, 14]) svg.append(element("rect", { x, y: 5, width: 4, height: 14, rx: 1.25 }));
  return svg;
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
export function nodeCard(node: PositionedNode, canvasId: string, index: number, signal: AbortSignal, onOpen: (scope: string) => void, onReview?: (sourceId: string) => void) {
  const preview = node.preview;
  const skippedPreview = !preview && canvasPreviewReserved(node) && node.state === "skipped";
  const unavailablePreview = !preview && canvasPreviewReserved(node) && (node.state === "succeeded" || node.value?.state === "known");
  const skipMessage = node.state === "skipped" ? node.skip_explanation?.message ?? "Reason not recorded." : null;
  const { settings_y, preview_y, preview_height } = canvasNodeGeometry(node);
  const footer = reviewFooter(node);
  const footerTop = footer ? node.height - footer.height : node.height;
  const action = onReview && node.review_action;
  const portsLabel = node.ports ? `, inputs: ${node.ports.inputs.map((port) => `${displayPortName(port.name)} (${port.type}, ${port.kind})`).join(", ") || "none"}, outputs: ${node.ports.outputs.map((port) => `${displayPortName(port.name)} (${port.type}, ${port.kind})`).join(", ") || "none"}` : node.pending ? "" : ", port metadata not recorded";
  const scopeLabel = node.kind === "workflow" ? `, imported workflow, ${node.child_count ?? 0} steps, ${node.failure_count ?? 0} failed` : "";
  const state = node.state === "waiting" && node.waiting_for ? `waiting for ${node.waiting_for}` :
    footer?.pending || node.state === "awaiting" ? cardStatus(node).text : node.state;
  const review = node.review_label ? `, ${node.review_label}` : "";
  const count = preview ? node.value ? `${preview.count} ${preview.kind}${preview.count === 1 ? "" : "s"}` : `${preview.count} ${preview.kind} ${preview.count === 1 ? "output" : "outputs"}` : "";
  const label = `${node.title}, ${state}${review}${footer?.pending && node.review_prompt ? `, ${node.review_prompt}` : ""}${scopeLabel}${portsLabel}${preview ? `, ${preview.label}, ${count}` : skippedPreview ? ", No preview to display." : unavailablePreview ? ", Preview unavailable." : ""}${skipMessage ? ` ${skipMessage}` : ""}`;
  const containsAction = node.kind === "workflow" || Boolean(action);
  const group = element("g", { transform: `translate(${node.x},${node.y})`, class: `fx-canvas-node${node.value ? " fx-canvas-value" : ""}${node.kind === "workflow" ? " fx-canvas-workflow" : node.kind === "boundary" ? " fx-canvas-boundary" : ""}`, role: containsAction ? "group" : "button", tabindex: 0, "data-node-id": node.id, "data-state": node.state, "aria-label": label, ...(containsAction ? { "aria-current": "false" } : { "aria-pressed": "false" }) });
  group.append(element("rect", { width: node.width, height: node.height, rx: node.value ? 5 : 11, class: node.pending ? "fx-canvas-node-box fx-canvas-pending" : "fx-canvas-node-box" }));
  if (footer?.pending) {
    group.classList.add("fx-canvas-review-pending");
    const right = node.width - 1, bottom = node.height - 1;
    group.append(element("path", { d: `M1 11Q1 1 11 1H${right - 10}Q${right} 1 ${right} 11V59H1Z`, class: "fx-canvas-review-header", "aria-hidden": "true" }));
    group.append(element("path", { d: `M1 ${footerTop}H${right}V${bottom - 10}Q${right} ${bottom} ${right - 10} ${bottom}H11Q1 ${bottom} 1 ${bottom - 10}Z`, class: "fx-canvas-review-footer", "aria-hidden": "true" }));
  }
  if (footer) group.append(element("line", { x1: 1, x2: node.width - 1, y1: footerTop, y2: footerTop, class: "fx-canvas-review-divider" }));
  if (node.value) {
    // A printed binding label belongs to the data sheet, inside its existing footprint.
    group.append(element("path", { d: `M5 1H${node.width - 5}Q${node.width - 1} 1 ${node.width - 1} 5V59H1V5Q1 1 5 1Z`, class: "fx-canvas-value-label", "aria-hidden": "true" }));
    group.append(createElement(Braces, { x: 15, y: 15, width: 16, height: 16, "stroke-width": 1.6, class: "fx-canvas-value-mark", "aria-hidden": "true" }));
  }
  group.append(element("text", { x: node.value ? 40 : 16, y: 27, class: "fx-canvas-node-title" }, short(node.title, node.value ? 38 : node.ports ? 42 : 28)));
  group.append(element("text", { x: node.value ? 40 : 16, y: 47, class: "fx-canvas-node-type" }, short(node.kind === "workflow" ? node.path ?? node.subtitle : node.subtitle, node.ports ? 51 : 34)));
  if (node.value) {
    group.setAttribute("aria-label", `${node.title}, ${node.subtitle}, ${short(valueStateLabel(node.value), 160)}`);
    group.setAttribute("data-value-id", node.value.id);
    group.append(element("line", { x1: 1, y1: 59, x2: node.width - 1, y2: 59, class: "fx-canvas-node-divider" }));
    for (const direction of ["input", "output"] as const) {
      const ports = direction === "input" ? node.ports?.inputs : node.ports?.outputs;
      if (ports?.length) group.append(element("rect", { x: direction === "input" ? -4 : node.width - 4, y: 24, width: 8, height: 8, rx: 2, class: "fx-canvas-value-socket" }));
    }
    const excerpt = valuePreviewLines(valueStateLabel(node.value));
    const content = element("foreignObject", { x: 16, y: 68, width: node.width - 32, height: Math.max(1, excerpt.lines.length) * 18 });
    const body = document.createElement("div");
    body.setAttribute("xmlns", "http://www.w3.org/1999/xhtml");
    body.className = `fx-canvas-value-content${node.value.state !== "known" ? " is-unavailable" : ""}`;
    body.style.setProperty("--value-lines", String(Math.max(1, excerpt.lines.length)));
    body.textContent = excerpt.lines.join("\n");
    content.append(body); group.append(content);
    if (excerpt.more) group.append(element("text", { x: 16, y: node.height - 12, class: "fx-canvas-value-more" }, "Read full value"));
  } else if (node.ports) {
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
        socket.append(element("rect", { x: direction === "input" ? -10 : node.width - 166, y: y - 12, width: 176, height: 32, rx: 3, class: "fx-canvas-port-hit" }));
        socket.append(port.kind === "artifact" ? element("circle", { cx: x, cy: y, r: 4.5, class: "fx-canvas-port-socket" })
          : port.kind === "fact" ? element("path", { d: `M${x} ${y - 5}l5 5l-5 5l-5 -5z`, class: "fx-canvas-port-socket" })
          : element("rect", { x: x - 4, y: y - 4, width: 8, height: 8, rx: 1, class: "fx-canvas-port-socket" }));
        socket.append(element("text", { x: textX, y: y + 3, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-name" }, short(displayPortName(port.name), 22)));
        socket.append(element("text", { x: textX, y: y + 16, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-type" }, short(`${port.kind === "parameter" ? "param · " : ""}${port.type}`, 25)));
        group.append(socket);
      }
      if (!ports.length) group.append(element("text", { x: textX, y: PORT_TOP + 3, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-type" }, "None"));
    }
    if (node.summaries?.length) {
      group.append(element("text", { x: 16, y: settings_y + 9, class: "fx-canvas-port-heading" }, "EFFECTIVE VALUES"));
      for (const [index, summary] of node.summaries.entries()) {
        group.append(element("text", { x: 16, y: settings_y + 29 + index * 34, class: "fx-canvas-summary-name" }, short(summary.name, 18)));
        const text = element("text", { x: 118, y: settings_y + 29 + index * 34, class: "fx-canvas-summary-value" }, short(summary.text.replace(/\s+/g, " "), 30));
        text.append(element("title", {}, summary.text));
        group.append(text);
      }
      const hidden = (node.summary_count ?? 0) - node.summaries.length;
      if (hidden > 0) group.append(element("text", { x: 16, y: settings_y + 24 + node.summaries.length * 34, class: "fx-canvas-value-more" }, `+${hidden} more in details`));
    } else if (node.ports.settings.length) {
      const settings = element("text", { x: 16, y: settings_y + 10, class: "fx-canvas-settings" }, short(`Settings · ${node.ports.settings.map((setting) => setting.name).join(", ")}`, 52));
      group.append(settings);
    }
  } else if (!node.pending && !preview) {
    group.append(element("text", { x: 16, y: 64, class: "fx-canvas-port-type" }, "Port metadata not recorded"));
  }
  if (node.route_label) group.append(element("text", { x: 16, y: preview_y - 7, class: "fx-canvas-node-route" }, short(node.route_label, 51)));
  const frame = { x: 16, y: preview_y, width: node.width - 32, height: preview_height };
  if (preview) {
    const clipId = `${canvasId}-preview-${index}`;
    const clip = element("clipPath", { id: clipId });
    clip.append(element("rect", { ...frame, rx: 6 }));
    group.append(clip);
    const backdrop = element("rect", { ...frame, rx: 6, fill: `url(#${canvasId}-checker)` });
    const unavailable = preview.kind === "model" ? "3D preview unavailable" : preview.kind === "video" ? "Browser preview unavailable" : "Image unavailable";
    const image = element("image", { ...frame, preserveAspectRatio: "xMidYMid meet", "clip-path": `url(#${clipId})`, role: "img", "aria-label": preview.label, class: `fx-canvas-node-image${preview.kind === "video" ? " fx-canvas-video-still" : preview.kind === "model" ? " fx-canvas-model-still" : ""}` });
    const fallback = element("text", { x: node.width / 2, y: frame.y + frame.height / 2 + (preview.kind === "image" ? 0 : 38), "text-anchor": "middle", "dominant-baseline": "middle", class: `fx-canvas-image-error${preview.kind === "model" ? " fx-canvas-model-error" : ""}`, visibility: "hidden" }, unavailable);
    if (preview.kind === "image") image.addEventListener("error", () => {
      image.setAttribute("visibility", "hidden");
      fallback.setAttribute("visibility", "visible");
      group.setAttribute("aria-label", `${label}, ${unavailable}: ${preview.label}`);
    }, { signal });
    const border = element("rect", { ...frame, rx: 6, class: "fx-canvas-image-border" });
    if (preview.kind === "image") {
      image.setAttribute("href", preview.url);
      group.append(backdrop, image, border, fallback);
    } else if (preview.kind === "model") {
      // A browser-rendered still keeps cards light; one explicitly activated HTML model
      // inspector is placed over this frame by the canvas, like the native-video player.
      backdrop.setAttribute("fill", "#f7f7f8");
      const model = element("g", { class: "fx-canvas-model", "data-digest": preview.digest });
      const center = { x: node.width / 2, y: frame.y + frame.height / 2 };
      const open = element("g", { class: "fx-canvas-model-open", role: "button", tabindex: 0, "aria-label": `Inspect 3D ${preview.label}`, "data-label": preview.label });
      // The poster is one action, with the cube as its visual affordance.
      open.append(element("title", {}, `Inspect 3D ${preview.label}`), element("rect", { ...frame, rx: 6, fill: "transparent" }),
        element("circle", { cx: center.x, cy: center.y, r: 18 }),
        createElement(Box, { x: center.x - 8, y: center.y - 8, width: 16, height: 16, "stroke-width": 1.5, "aria-hidden": "true" }));
      model.append(backdrop, image, border, fallback, open);
      group.append(model);
    } else {
      // A video shows its first frame, which the canvas sets once the card is in view, and a play
      // control; the canvas plays it over the frame (video.ts). The backdrop takes the pointer, so
      // resting anywhere on the frame plays it muted.
      const video = element("g", { class: "fx-canvas-video", "data-digest": preview.digest });
      const play = element("g", { class: "fx-canvas-play", role: "button", tabindex: 0, "aria-label": `Play ${preview.label}`, "data-label": preview.label });
      const center = { x: node.width / 2, y: frame.y + frame.height / 2 };
      play.append(element("title", {}, `Play ${preview.label}`), element("circle", { cx: center.x, cy: center.y, r: 18 }),
        mediaGlyph("play", { x: center.x - 8, y: center.y - 8, width: 16, height: 16, class: "fx-canvas-video-play-glyph" }),
        createElement(RotateCw, { x: center.x - 9, y: center.y - 9, width: 18, height: 18, "stroke-width": 2, "aria-hidden": "true", class: "fx-canvas-video-retry-glyph" }),
        element("text", { x: center.x, y: center.y - 28, "text-anchor": "middle", class: "fx-canvas-video-retry-label", "aria-hidden": "true" }, "Try playback"));
      const unavailableIcon = createElement(VideoOff, { x: center.x - 14, y: center.y - 14, width: 28, height: 28, "stroke-width": 1.5, "aria-hidden": "true", class: "fx-canvas-video-disabled-icon" });
      video.append(backdrop, image);
      if (preview.poster) {
        const poster = element("image", { ...frame, preserveAspectRatio: "xMidYMid meet", "clip-path": `url(#${clipId})`, role: "img", "aria-label": preview.poster.label, class: "fx-canvas-node-image fx-canvas-video-poster" });
        poster.addEventListener("load", () => { video.classList.add("has-poster"); video.classList.remove("poster-failed"); }, { signal });
        poster.addEventListener("error", () => { video.classList.remove("has-poster"); video.classList.add("poster-failed"); }, { signal });
        poster.setAttribute("href", preview.poster.url);
        video.append(poster);
      }
      video.append(border, fallback, play, unavailableIcon);
      group.append(video);
    }
    group.append(element("text", { x: node.width - 16, y: footer ? footerTop - 7 : node.height - 22, "text-anchor": "end", class: "fx-canvas-image-count" }, count));
  } else if (canvasPreviewReserved(node)) {
    // Only a started step shimmers: the declared file may be on its way.
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
    if (skippedPreview) {
      const center = { x: node.width / 2, y: frame.y + frame.height / 2 };
      const empty = element("g", { class: "fx-canvas-skipped-preview", "aria-hidden": "true" });
      empty.append(
        element("rect", { ...frame, rx: 6, class: "fx-canvas-skipped-preview-background" }),
        createElement(CircleSlash, { x: center.x - 12, y: center.y - 36, width: 24, height: 24, "stroke-width": 1.5, "aria-hidden": "true" }),
        element("text", { x: center.x, y: center.y + 8, "text-anchor": "middle", class: "fx-canvas-skipped-preview-title" }, "Skipped"),
        ...skipPreviewLines(skipMessage!).map((line, at) => element("text", { x: center.x, y: center.y + 28 + at * 14, "text-anchor": "middle", class: "fx-canvas-skipped-preview-message" }, line)),
      );
      group.append(empty);
    }
    if (unavailablePreview) group.append(element("text", {
      x: node.width / 2, y: frame.y + frame.height / 2, "text-anchor": "middle", "dominant-baseline": "middle", class: "fx-canvas-image-error",
    }, node.value ? "File preview unavailable" : "Preview unavailable"));
    group.append(element("rect", { ...frame, rx: 6, class: "fx-canvas-image-border" }));
  }
  const prompt = footer?.pending ? node.review_prompt?.trim() : undefined;
  const statusY = footer ? footerTop + (prompt ? 25 : footer.height / 2 + 4) : node.height - 22;
  const deadline = footer?.pending && node.review_deadline ? Date.parse(node.review_deadline) : NaN;
  const showCountdown = Number.isFinite(deadline);
  const showStateIcon = !footer?.pending || showCountdown;
  const statusX = showStateIcon ? 32 : 16;
  if (!node.value) {
    if (showStateIcon) group.append(stateIconElement(showCountdown ? "waiting" : node.state, 15, statusY - 10, 12));
    // The canvas updates only the time text, so the card and its Review button retain focus.
    const status = cardStatus(node, node.ports ? 18 : 12);
    const label = status.from === undefined
      ? element("text", { x: statusX, y: statusY, class: "fx-canvas-node-state" }, status.text)
      : element("text", { x: statusX, y: statusY, class: "fx-canvas-node-state", "data-started": status.from, "data-prefix": status.text }, status.text + runningLabel(status.from, Date.now()));
    if (showCountdown) label.append(
      element("tspan", {}, " · "),
      element("tspan", { class: "fx-canvas-review-countdown", "data-review-countdown": deadline, role: "timer" }, countdownLabel(deadline)),
    );
    group.append(label);
  }
  if (footer) {
    const width = action === "review" ? 73 : 89;
    const x = node.width - 16 - width;
    if (prompt) {
      const line = element("foreignObject", { x: statusX, y: footerTop + 33, width: (action ? x - 10 : node.width - 16) - statusX, height: 22, "aria-hidden": "true" });
      const text = document.createElement("div");
      text.className = "fx-canvas-review-prompt";
      text.title = prompt;
      text.textContent = prompt;
      line.append(text);
      group.append(line);
    }
    if (action && onReview) {
      const name = action === "review" ? "Review" : action === "view_answer" ? "View answer" : "View outcome";
      const button = element("g", { transform: `translate(${x},${footerTop + (footer.height - 30) / 2})`, class: "fx-canvas-review-action",
        role: "button", tabindex: 0, "aria-label": action === "review" ? `Review ${node.title}` : `${name} for ${node.title}`, "aria-controls": "fx-review-panel" });
      button.append(element("rect", { width, height: 30, rx: 6 }), element("text", { x: width / 2, y: 19, "text-anchor": "middle" }, name));
      const activate = (event: Event) => {
        event.preventDefault();
        event.stopPropagation();
        if (button.getAttribute("tabindex") === "0") onReview(node.source_id);
      };
      button.addEventListener("click", activate, { signal });
      button.addEventListener("keydown", event => { if (event.key === "Enter" || event.key === " ") activate(event); }, { signal });
      group.append(button);
    }
  }
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
