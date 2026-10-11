import { describe, expect, test } from "bun:test";
import type { ShowcaseSlug } from "../../lib/showcases/contracts";
import { PortraitDemoController, replacePatchRgb, selectExpression } from "./motion-controller";

describe("native face composition", () => {
  test("replaces selected RGB without changing alpha or neighboring pixels", () => {
    const frame = new Uint8ClampedArray([10, 20, 30, 128, 40, 50, 60, 0, 70, 80, 90, 255]);
    replacePatchRgb(frame, [[0, 100, 110, 120], [8, 130, 140, 150]]);
    expect([...frame]).toEqual([100, 110, 120, 128, 40, 50, 60, 0, 130, 140, 150, 255]);
  });

  test("uses exact segment boundaries and repeats the authored timeline", () => {
    const timeline = [{ eyes: "rest", mouth: "rest", duration_ms: 1000 }, { eyes: "closed", mouth: "smile", duration_ms: 120 }];
    expect(selectExpression(timeline, .999).eyes).toBe("rest");
    expect(selectExpression(timeline, 1)).toMatchObject({ eyes: "closed", mouth: "smile" });
    expect(selectExpression(timeline, 1.12).eyes).toBe("rest");
    expect(selectExpression(timeline, -.1).eyes).toBe("closed");
  });
});

class DemoElement extends EventTarget {
  readonly dataset: Record<string, string> = {};
  readonly attributes = new Map<string, string>();
  readonly style = { setProperty: (name: string, value: string) => { this.attributes.set(name, value); } };
  hidden = false;
  disabled = false;
  checked = true;
  value = "rest";
  max = "8";
  private text = "";
  onText?: (text: string) => void;
  get textContent() { return this.text; }
  set textContent(text: string) { this.text = text; this.onText?.(text); }
  setAttribute(name: string, value: string) { this.attributes.set(name, value); }
}

class DemoCanvas extends DemoElement {
  width = 896;
  height = 896;
  readonly draws: unknown[][] = [];
  readonly reads: number[][] = [];
  readonly writes: { data: Uint8ClampedArray; x: number; y: number }[] = [];
  pixels?: Uint8ClampedArray;
  readonly context = {
    clearRect() {},
    drawImage: (...args: unknown[]) => { this.draws.push(args); },
    getImageData: (x: number, y: number, width: number, height: number) => {
      this.reads.push([x, y, width, height]);
      return { data: this.pixels ? new Uint8ClampedArray(this.pixels) : new Uint8ClampedArray(width * height * 4) };
    },
    putImageData: (image: { data: Uint8ClampedArray }, x: number, y: number) => { this.writes.push({ data: new Uint8ClampedArray(image.data), x, y }); },
  };
  getContext() { return this.context; }
}

class DemoVideo extends DemoElement {
  readonly requests: string[] = [];
  private source = "";
  plays = 0;
  loads = 0;
  pauses = 0;
  readyState = 2;
  currentTime = 0;
  playbackRate = 1;
  playResult?: Promise<void>;
  get src() { return this.source; }
  set src(value: string) { this.source = value; this.requests.push(value); }
  play() { this.plays++; return this.playResult ?? Promise.resolve(); }
  pause() { this.pauses++; }
  load() { this.loads++; }
}

/** Exercise the actual controller without a browser, network, or rendered pixel assumptions. */
async function portraitFixture(body: boolean, wrongCanonicalSize = false, sourceSize: [number, number] = [896, 896], playResult?: Promise<void>, options: {
  timeline?: { eyes: string; mouth: string; duration_ms: number }[];
  patchSize?: [number, number];
  offset?: [number, number];
  patchPixels?: Uint8ClampedArray;
} = {}) {
  const video = new DemoVideo(), canvas = new DemoCanvas(), zoom = new DemoCanvas();
  video.playResult = playResult;
  const elements = new Map<string, DemoElement>([
    ["[data-body]", video], ["[data-portrait]", canvas], ["[data-face-zoom]", zoom],
    ...["data-stage", "data-scrub", "data-play", "data-reset", "data-speed", "data-automatic", "data-eyes", "data-mouth", "data-selection", "data-time", "data-status", "data-fallback", "data-portrait-layout", "data-body-controls", "data-face-detail"].map(name => [`[${name}]`, new DemoElement()] as const),
  ]);
  elements.get("[data-eyes]")!.disabled = true;
  elements.get("[data-mouth]")!.disabled = true;
  const modes = ["combined", "body", "face"].map(mode => { const element = new DemoElement(); element.dataset.mode = mode; return element; });
  const root = {
    querySelector: (selector: string) => elements.get(selector),
    querySelectorAll: (selector: string) => selector === "[data-mode]" ? modes
      : selector === '[data-mode="body"], [data-mode="combined"]' ? modes.slice(0, 2)
      : selector === "[data-eyes], [data-mouth]" ? [elements.get("[data-eyes]"), elements.get("[data-mouth]")]
      : selector === "video" ? [video]
      : selector === "button, input, select" ? [...elements.values()]
      : [],
  };
  const globals = globalThis as unknown as Record<string, unknown>;
  const previous = new Map<string, PropertyDescriptor | undefined>();
  let tick: FrameRequestCallback | undefined;
  let settled!: () => void;
  const ready = new Promise<void>(resolve => { settled = resolve; });
  const install = (name: string, value: unknown) => {
    previous.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  };
  install("matchMedia", () => ({ matches: false }));
  install("requestAnimationFrame", (callback: FrameRequestCallback) => { tick = callback; settled(); return 1; });
  install("cancelAnimationFrame", () => {});
  install("document", { hidden: false, createElement: () => { const scratch = new DemoCanvas(); scratch.pixels = options.patchPixels; return scratch; } });
  elements.get("[data-status]")!.onText = text => { if (text) settled(); };
  const resources = ["assets/face-manifest.json", "assets/canonical.png", "assets/animation.webp", ...(body ? ["assets/body.webm"] : [])];
  const face = {
    kind: "portrait-patches-v1", patch_application: "replace_selected_rgb_preserve_original_alpha", feather_already_baked: true,
    source_size: sourceSize, patch_size: options.patchSize ?? [1, 1], offset_xy: options.offset ?? [100, 100],
    timeline: options.timeline ?? [{ eyes: "rest", mouth: "rest", duration_ms: 1000 }],
    patches: [["canvas_left_eye", "eyes_half"], ["canvas_right_eye", "eyes_half"], ["canvas_left_eye", "eyes_closed"], ["canvas_right_eye", "eyes_closed"], ["mouth", "mouth_smile"], ["mouth", "mouth_a"]].map(([feature_id, state_id]) => ({ feature_id, state_id, key: `${state_id}-${feature_id}` })),
  };
  const canonical = { naturalWidth: wrongCanonicalSize ? 720 : sourceSize[0], naturalHeight: sourceSize[1] };
  class Controller extends PortraitDemoController {
    protected override async json<T>(_slug: ShowcaseSlug, path: string, parse: (value: unknown) => T): Promise<T> {
      return parse(path === "manifest.json" ? { kind: "portrait-motion-demo-v1", resources } : face);
    }
    protected override async image(path: string): Promise<HTMLImageElement> {
      return (path === "assets/canonical.png" ? canonical : { naturalWidth: face.patch_size[0], naturalHeight: face.patch_size[1] }) as HTMLImageElement;
    }
  }
  const controller = new Controller(root as unknown as HTMLElement, "https://assets.example/portrait");
  await ready;
  return {
    video, canvas, zoom, canonical, elements,
    draw(now = 0) { tick?.(now); },
    dispose() {
      controller.dispose();
      for (const [name, descriptor] of previous) {
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else delete globals[name];
      }
    },
  };
}

test("face-only portraits keep native canvas dimensions and never request or play body media", async () => {
  const fixture = await portraitFixture(false);
  try {
    expect(fixture.video.requests).toEqual([]);
    expect(fixture.video.loads).toBe(0);
    expect(fixture.video.plays).toBe(0);
    expect([fixture.canvas.width, fixture.canvas.height]).toEqual([896, 896]);
    expect(fixture.elements.get("[data-stage]")!.attributes.get("--portrait-aspect")).toBe("896 / 896");
    expect(fixture.elements.get("[data-portrait-layout]")!.dataset.faceOnly).toBe("true");
    expect(fixture.elements.get("[data-body-controls]")!.hidden).toBe(true);
    expect(fixture.elements.get("[data-face-detail]")!.hidden).toBe(true);
    fixture.draw();
    expect(fixture.canvas.draws[0]).toEqual([fixture.canonical, 0, 0]);
    expect(fixture.zoom.draws).toEqual([]);
    const automatic = fixture.elements.get("[data-automatic]")!;
    automatic.checked = false;
    automatic.dispatchEvent(new Event("change"));
    fixture.elements.get("[data-eyes]")!.value = "eyes_closed";
    fixture.elements.get("[data-mouth]")!.value = "mouth_a";
    fixture.draw();
    expect(fixture.elements.get("[data-eyes]")!.disabled).toBe(false);
    expect(fixture.elements.get("[data-mouth]")!.disabled).toBe(false);
    expect(fixture.elements.get("[data-selection]")!.textContent).toBe("Closed eyes · Open A");
  } finally { fixture.dispose(); }
});

test("legacy listed body media stays available and failures leave functional face-only controls", async () => {
  const fixture = await portraitFixture(true, false, [720, 1280]);
  try {
    expect(fixture.video.requests).toEqual(["https://assets.example/portrait/assets/body.webm"]);
    expect(fixture.video.loads).toBe(1);
    expect(fixture.video.plays).toBe(1);
    expect(fixture.elements.get("[data-portrait-layout]")!.dataset.faceOnly).toBe("false");
    expect(fixture.elements.get("[data-body-controls]")!.hidden).toBe(false);
    fixture.draw();
    expect(fixture.canvas.draws[0]).toEqual([fixture.video, 0, 0]);
    expect(fixture.zoom.draws.length).toBe(1);
    fixture.draw(16);
    expect(fixture.canvas.draws.length).toBe(2);
    expect(fixture.canvas.reads.length).toBe(2);
    fixture.video.dispatchEvent(new Event("error"));
    expect(fixture.elements.get("[data-portrait-layout]")!.dataset.faceOnly).toBe("true");
    expect(fixture.elements.get("[data-body-controls]")!.hidden).toBe(true);
    expect(fixture.elements.get("[data-automatic]")!.disabled).toBe(false);
    expect(fixture.elements.get("[data-stage]")!.attributes.get("--portrait-aspect")).toBe("720 / 1280");
    expect(fixture.elements.get("[data-status]")!.textContent).toContain("Face controls remain available");
  } finally { fixture.dispose(); }
});

test("mismatched canonical portrait dimensions fail clearly before any body request", async () => {
  const fixture = await portraitFixture(true, true);
  try {
    expect(fixture.elements.get("[data-status]")!.textContent).toContain("dimensions do not match");
    expect(fixture.canvas.hidden).toBe(true);
    expect(fixture.video.requests).toEqual([]);
  } finally { fixture.dispose(); }
});

test("late body playback rejection cannot pause the independent face clock after video failure", async () => {
  let reject!: (reason: Error) => void;
  const playResult = new Promise<void>((_resolve, fail) => { reject = fail; });
  const fixture = await portraitFixture(true, false, [720, 1280], playResult);
  try {
    fixture.draw(1000);
    fixture.video.dispatchEvent(new Event("error"));
    reject(new Error("Body decoding failed."));
    await Promise.resolve();
    expect(fixture.elements.get("[data-play]")!.textContent).toBe("Pause");
    expect(fixture.elements.get("[data-play]")!.disabled).toBe(false);
    fixture.draw(1100);
    expect(fixture.elements.get("[data-time]")!.textContent).toBe("0.10 / 1.00 s");
    expect(fixture.canvas.draws.at(-1)).toEqual([fixture.canonical, 0, 0]);
  } finally { fixture.dispose(); }
});

test("native face frames recompose only for expression transitions or dirty interactions while the clock stays live", async () => {
  const fixture = await portraitFixture(false, false, [2048, 2048], undefined, {
    timeline: [
      { eyes: "rest", mouth: "rest", duration_ms: 200 },
      { eyes: "eyes_closed", mouth: "rest", duration_ms: 200 },
      { eyes: "rest", mouth: "rest", duration_ms: 200 },
    ],
  });
  try {
    fixture.draw(1000);
    expect(fixture.canvas.reads.length).toBe(1);
    fixture.draw(1100);
    expect(fixture.canvas.reads.length).toBe(1);
    expect(fixture.elements.get("[data-time]")!.textContent).toBe("0.10 / 0.60 s");
    fixture.draw(1200);
    expect(fixture.canvas.reads.length).toBe(2);
    expect(fixture.elements.get("[data-selection]")!.textContent).toBe("Closed eyes · Rest");
    fixture.draw(1300);
    expect(fixture.canvas.reads.length).toBe(2);
    fixture.draw(1400);
    expect(fixture.canvas.reads.length).toBe(3);
    expect(fixture.canvas.writes.length).toBe(3);
    expect(fixture.elements.get("[data-selection]")!.textContent).toBe("Open eyes · Rest");

    fixture.elements.get("[data-play]")!.dispatchEvent(new Event("click"));
    fixture.draw(1500);
    const automatic = fixture.elements.get("[data-automatic]")!;
    automatic.checked = false;
    automatic.dispatchEvent(new Event("change"));
    const mouth = fixture.elements.get("[data-mouth]")!;
    mouth.value = "mouth_smile";
    mouth.dispatchEvent(new Event("change"));
    fixture.draw(1600);
    expect(fixture.canvas.reads.length).toBe(5);
    expect(fixture.elements.get("[data-selection]")!.textContent).toBe("Open eyes · Smile");
    const scrub = fixture.elements.get("[data-scrub]")!;
    scrub.value = ".3";
    scrub.dispatchEvent(new Event("input"));
    fixture.draw(1700);
    expect(fixture.canvas.reads.length).toBe(6);
    expect(fixture.elements.get("[data-time]")!.textContent).toBe("0.30 / 0.60 s");
    fixture.elements.get("[data-reset]")!.dispatchEvent(new Event("click"));
    fixture.draw(1800);
    expect(fixture.canvas.reads.length).toBe(7);
    expect(fixture.elements.get("[data-time]")!.textContent).toBe("0.00 / 0.60 s");
    fixture.draw(1900);
    expect(fixture.canvas.reads.length).toBe(7);
  } finally { fixture.dispose(); }
});

test("native patches extending above and left of the source clip without shifting selected pixels or source alpha", async () => {
  const patchPixels = new Uint8ClampedArray(6 * 5 * 4);
  for (let y = 0; y < 5; y++) for (let x = 0; x < 6; x++) patchPixels.set([x + 1, y + 1, 90, 255], (y * 6 + x) * 4);
  const fixture = await portraitFixture(false, false, [4, 3], undefined, {
    patchSize: [6, 5], offset: [-1, -1], patchPixels,
    timeline: [{ eyes: "eyes_closed", mouth: "rest", duration_ms: 1000 }],
  });
  try {
    fixture.canvas.pixels = new Uint8ClampedArray(4 * 3 * 4);
    for (let index = 3; index < fixture.canvas.pixels.length; index += 4) fixture.canvas.pixels[index] = 128;
    fixture.draw();
    expect(fixture.canvas.reads).toEqual([[0, 0, 4, 3]]);
    const [{ data, x, y }] = fixture.canvas.writes;
    expect([x, y]).toEqual([0, 0]);
    expect([...data.slice(0, 4)]).toEqual([2, 2, 90, 128]);
    expect([...data.slice(-4)]).toEqual([5, 4, 90, 128]);
    expect([...data].filter((_value, index) => index % 4 === 3)).toEqual(new Array(12).fill(128));
  } finally { fixture.dispose(); }
});
