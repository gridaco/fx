import { parseCharacter, parseWorld } from "../../lib/showcases/contracts";
import { loadShowcaseJson } from "../../lib/showcases/load";
import { FlatModelPreview } from "./flat-model-preview";

type WorldEntity = {
  entity: {
    entity_id: string; display_name: string; primary_class: string; entity_kind: string;
    summary: string; how_it_works_or_lives: string; present_tension: string;
    facts: { claim: string }[];
  };
  image: string;
  status: string;
  concept: { audience_question?: string; scene_premise?: string };
  relationships: { other_entity_id: string; other_display_name: string; summary: string }[];
  review?: { verdict: string; what_the_image_teaches: string; blocking_findings: string[]; advisory_findings: string[] };
};
type WorldManifest = { entities: WorldEntity[] };
type ModelAsset = { url: string; stage: string; bytes: number };
type ImageAsset = { url: string; label: string; source_label: string };
type CharacterManifest = { models: ModelAsset[]; images: ImageAsset[]; result?: { status: string }; runtime_qualified: boolean };
type ModelElement = HTMLElement & {
  src: string; availableAnimations: string[]; animationName: string; paused: boolean;
  cameraOrbit: string; cameraTarget: string; fieldOfView: string; autoRotate: boolean;
  timeScale: number;
  play(): void; pause(): void; jumpCameraToGoal(): void;
};

const buttonClass = "rounded-md border px-3 py-2 text-xs transition-colors hover:bg-muted aria-pressed:border-foreground aria-pressed:bg-foreground aria-pressed:text-background disabled:opacity-40";

function element<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, className?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (text) node.textContent = text;
  if (className) node.className = className;
  return node;
}

class ExhibitController {
  protected readonly abort = new AbortController();
  protected readonly base: URL;
  constructor(protected readonly root: HTMLElement, assetBase: string) {
    this.base = new URL(`${assetBase.replace(/\/+$/, "")}/`, document.baseURI);
  }
  protected query<T extends HTMLElement = HTMLElement>(selector: string): T {
    const node = this.root.querySelector<T>(selector);
    if (!node) throw new Error(`Missing showcase element: ${selector}`);
    return node;
  }
  protected asset(path: string): string { return new URL(path, this.base).href; }
  protected text(selector: string, text: string): void { this.query(selector).textContent = text; }
  protected async manifest<T>(slug: "world" | "character-3d", parse: (value: unknown) => T): Promise<T> {
    return loadShowcaseJson(slug, this.base.href, "manifest.json", parse, this.abort.signal);
  }
  protected failed(error: unknown): void {
    if (!this.abort.signal.aborted) this.text("[data-demo-status]", error instanceof Error ? error.message : "Showcase assets could not load.");
  }
  dispose(): void { this.abort.abort(); }
}

/** One selected entity at a time, with links through the world's relationships. */
export class WorldGalleryController extends ExhibitController {
  private data?: WorldManifest;
  private selected = "";

  constructor(root: HTMLElement, assetBase: string) {
    super(root, assetBase);
    root.addEventListener("click", this.click, { signal: this.abort.signal });
    this.query("[data-world-nav]").addEventListener("keydown", this.keydown, { signal: this.abort.signal });
    this.query("[data-world-image]").addEventListener("error", () => this.text("[data-demo-status]", "This illustration could not load. Try another entity."), { signal: this.abort.signal });
    void this.load().catch((error: unknown) => this.failed(error));
  }

  private async load(): Promise<void> {
    const data = await this.manifest("world", parseWorld);
    if (this.abort.signal.aborted) return;
    this.data = data;
    const nav = this.query("[data-world-nav]");
    nav.replaceChildren(...data.entities.map((item) => {
      const button = element("button", undefined, "min-w-0 rounded-lg text-left opacity-60 transition-opacity hover:opacity-100 aria-pressed:opacity-100");
      button.type = "button";
      button.dataset.worldEntity = item.entity.entity_id;
      const image = element("img", undefined, "aspect-[4/3] w-full rounded-md object-cover");
      image.src = this.asset(item.image); image.alt = ""; image.loading = "lazy";
      button.append(image, element("span", item.entity.display_name, "mt-2 block truncate text-xs"));
      return button;
    }));
    if (!data.entities.length) throw new Error("This world has no recorded entities.");
    this.select(data.entities[0].entity.entity_id);
    const admitted = data.entities.filter((item) => item.status === "admitted").length;
    this.text("[data-demo-status]", `${data.entities.length} illustrated studies · ${admitted} passed the workflow's image judge`);
  }

  private select(id: string): void {
    const item = this.data?.entities.find((candidate) => candidate.entity.entity_id === id);
    if (!item) return;
    this.selected = id;
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-world-nav] [data-world-entity]")) {
      const selected = button.dataset.worldEntity === id;
      button.setAttribute("aria-pressed", String(selected));
      button.tabIndex = selected ? 0 : -1;
    }
    const image = this.query<HTMLImageElement>("[data-world-image]");
    image.src = this.asset(item.image); image.alt = `${item.entity.display_name}: ${item.concept.scene_premise || item.entity.summary}`;
    this.text("[data-world-name]", item.entity.display_name);
    this.text("[data-world-kind]", `${item.entity.primary_class} · ${item.entity.entity_kind}`);
    this.text("[data-world-verdict]", item.status === "admitted" ? "Image review passed" : "Study · image review rejected");
    this.text("[data-world-summary]", item.entity.summary);
    this.text("[data-world-life]", item.entity.how_it_works_or_lives);
    this.text("[data-world-tension]", item.entity.present_tension);
    this.text("[data-world-lesson]", item.concept.audience_question || "");
    this.text("[data-world-scene]", item.concept.scene_premise || "");
    this.query("[data-world-facts]").replaceChildren(...item.entity.facts.map((fact) => element("li", fact.claim)));
    const connected = new Map(item.relationships.map((relationship) => [relationship.other_entity_id, relationship]));
    this.query("[data-world-relations]").replaceChildren(...Array.from(connected.values()).map((relationship) => {
      const button = element("button", `${relationship.other_display_name} →`, buttonClass);
      button.type = "button"; button.dataset.worldEntity = relationship.other_entity_id; button.title = relationship.summary;
      return button;
    }));
    this.text("[data-world-review]", item.review?.what_the_image_teaches || "No independent image review was recorded.");
    this.query("[data-world-findings]").replaceChildren(...[
      ...(item.review?.blocking_findings || []).map((finding) => element("li", `Blocking finding: ${finding}`)),
      ...(item.review?.advisory_findings || []).map((finding) => element("li", finding)),
    ]);
  }

  private click = (event: MouseEvent): void => {
    const button = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-world-entity]") : null;
    if (button?.dataset.worldEntity) this.select(button.dataset.worldEntity);
  };

  private keydown = (event: KeyboardEvent): void => {
    const buttons = Array.from(this.query("[data-world-nav]").querySelectorAll<HTMLButtonElement>("button"));
    const current = buttons.findIndex((button) => button.dataset.worldEntity === this.selected);
    const next = event.key === "ArrowRight" ? (current + 1) % buttons.length
      : event.key === "ArrowLeft" ? (current + buttons.length - 1) % buttons.length
      : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
    if (next === null || !buttons[next]) return;
    event.preventDefault();
    this.select(buttons[next].dataset.worldEntity!); buttons[next].focus();
  };
}

let viewerReady: Promise<void> | undefined;

/** The supplied bundle contains model-viewer and every decoder it needs. */
function loadModelViewer(base: URL): Promise<void> {
  if (customElements.get("model-viewer")) return Promise.resolve();
  return viewerReady ??= (async () => {
    const runtime = window as unknown as {
      MeshoptDecoder: unknown;
      ModelViewerElement: { dracoDecoderLocation: string; ktx2TranscoderLocation: string; meshoptDecoderLocation: string };
    };
    const meshopt = await import(/* webpackIgnore: true */ new URL("vendor/meshopt_decoder.module.js", base).href) as { MeshoptDecoder: unknown };
    runtime.MeshoptDecoder = meshopt.MeshoptDecoder;
    runtime.ModelViewerElement = {
      dracoDecoderLocation: new URL("vendor/draco/", base).href,
      ktx2TranscoderLocation: new URL("vendor/basis/", base).href,
      meshoptDecoderLocation: new URL("vendor/meshopt-ready.js", base).href,
    };
    await import(/* webpackIgnore: true */ new URL("vendor/model-viewer.min.js", base).href);
    await customElements.whenDefined("model-viewer");
  })().catch((error: unknown) => {
    viewerReady = undefined;
    throw error;
  });
}

const clipNames = ["rest", "shoulder_raise", "elbow_bend", "knee_bend", "wrist_bend", "cheer"];
const label = (value: string): string => value.replaceAll("_", " ").replace(/^./, (first) => first.toUpperCase());

/** Model loading, stage selection and animation stay independent of React. */
export class CharacterModelController extends ExhibitController {
  private readonly model: ModelElement;
  private readonly preview = new FlatModelPreview();
  private currentModelUrl = "";
  private expectedPreviewUrl = "";
  private data?: CharacterManifest;
  private ready = false;
  private currentClip = "cheer";
  private currentStage = "rig";
  private models = new Map<string, ModelAsset>();

  constructor(root: HTMLElement, assetBase: string) {
    super(root, assetBase);
    this.model = this.query<ModelElement>("[data-character-model]");
    const { signal } = this.abort;
    root.addEventListener("click", this.click, { signal });
    this.query<HTMLInputElement>("[data-model-rotate]").addEventListener("change", (event) => {
      this.model.autoRotate = (event.currentTarget as HTMLInputElement).checked;
    }, { signal });
    this.query<HTMLSelectElement>("[data-model-evidence]").addEventListener("change", this.showEvidence, { signal });
    this.model.addEventListener("load", this.loaded, { signal });
    this.model.addEventListener("error", () => {
      if (!this.expectedPreviewUrl || this.model.src !== this.expectedPreviewUrl) return;
      this.ready = false; this.currentModelUrl = ""; this.expectedPreviewUrl = "";
      this.text("[data-model-loading]", "The model could not load. Select a stage to retry.");
      this.syncClips();
    }, { signal });
    void this.load().catch((error: unknown) => {
      this.failed(error);
      if (!signal.aborted) this.text("[data-model-loading]", "The 3D viewer could not load.");
    });
  }

  override dispose(): void {
    this.model.pause?.();
    this.model.src = "";
    this.preview.dispose();
    super.dispose();
  }

  private async load(): Promise<void> {
    const data = await this.manifest("character-3d", parseCharacter);
    await loadModelViewer(this.base);
    if (this.abort.signal.aborted) return;
    this.data = data;
    const mesh = data.models.find((item) => item.stage.includes("normalize"));
    const assembly = data.models.find((item) => item.stage.includes("assembly"));
    const rig = data.models.find((item) => item.stage === "result") || data.models.at(-1);
    if (mesh) this.models.set("mesh", mesh);
    if (assembly) this.models.set("assembly", assembly);
    if (rig) this.models.set("rig", rig);
    const stages = this.query("[data-model-stages]");
    stages.replaceChildren(...["reference", ...this.models.keys()].map((stage) => {
      const button = element("button", stage === "rig" ? "Rigged character" : label(stage), buttonClass);
      button.type = "button"; button.dataset.modelStage = stage;
      return button;
    }));
    this.query("[data-model-clips]").replaceChildren(...clipNames.map((name) => {
      const button = element("button", label(name), buttonClass);
      button.type = "button"; button.dataset.modelClip = name; button.disabled = true;
      return button;
    }));
    const reference = data.images.find((image) => image.source_label.includes("canonical"));
    if (reference) this.query<HTMLImageElement>("[data-model-reference]").src = this.asset(reference.url);
    this.query("[data-model-images]").replaceChildren(...data.images.slice(0, 3).map((item) => {
      const figure = element("figure");
      const link = element("a"); link.href = this.asset(item.url); link.target = "_blank"; link.rel = "noopener";
      const image = element("img", undefined, "aspect-[3/4] w-full rounded-md bg-muted object-contain");
      image.src = this.asset(item.url); image.alt = item.source_label.includes("canonical") ? "Character reference" : item.label; image.loading = "lazy";
      link.append(image);
      figure.append(link, element("figcaption", item.source_label.includes("canonical") ? "Character reference" : label(item.label.split(" / ").at(-1) || item.label), "mt-2 text-xs text-muted-foreground"));
      return figure;
    }));
    const evidence = this.query<HTMLSelectElement>("[data-model-evidence]");
    evidence.replaceChildren(...data.images.filter((item) => item.source_label.startsWith("atlas")).map((item) => {
      const option = element("option", item.label); option.value = item.url; return option;
    }));
    this.showEvidence();
    this.selectStage(rig ? "rig" : this.models.keys().next().value || "reference");
    this.text("[data-demo-status]", `Pipeline result: ${data.result?.status || "recorded"} · ${data.runtime_qualified ? "Runtime qualified" : "Game and runtime qualification pending"}`);
  }

  private selectStage(stage: string): void {
    this.currentStage = stage;
    const reference = stage === "reference";
    this.model.hidden = reference;
    this.model.style.display = reference ? "none" : "block";
    this.query("[data-model-reference]").hidden = !reference;
    this.query("[data-model-loading]").hidden = reference;
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-model-stage]")) button.setAttribute("aria-pressed", String(button.dataset.modelStage === stage));
    if (reference) {
      if (this.ready) this.model.pause();
      this.text("[data-model-caption]", "The generated character reference anchors the front and back views used to build the mesh.");
    } else {
      const asset = this.models.get(stage);
      if (!asset) return;
      const src = this.asset(asset.url);
      if (this.currentModelUrl !== src) {
        this.ready = false; this.currentModelUrl = src; this.expectedPreviewUrl = "";
        this.model.src = ""; this.model.style.visibility = "hidden";
        this.text("[data-model-loading]", "Loading model…");
        void this.loadModel(asset, src).catch((error: unknown) => {
          if (this.abort.signal.aborted || this.currentModelUrl !== src) return;
          this.currentModelUrl = "";
          this.failed(error); this.text("[data-model-loading]", "The model could not load. Select a stage to retry.");
        });
      } else if (this.ready) this.play(this.currentClip);
      this.text("[data-model-caption]", `${label(stage)} · ${(asset.bytes / 1048576).toFixed(1)} MB GLB · ${stage === "rig" ? "Six clips demonstrate the rig; locomotion and facial animation are not included." : "An intermediate model from the same recorded workflow."}`);
    }
    this.syncClips();
  }

  private async loadModel(asset: ModelAsset, src: string): Promise<void> {
    const url = await this.preview.select(src, asset.bytes);
    if (!url || this.abort.signal.aborted || this.currentModelUrl !== src) return;
    this.expectedPreviewUrl = url;
    this.model.src = url;
  }

  private loaded = (event: Event): void => {
    const url = (event as CustomEvent<{ url?: string }>).detail?.url;
    if (!this.expectedPreviewUrl || url !== this.expectedPreviewUrl || this.model.src !== this.expectedPreviewUrl) return;
    this.model.style.visibility = "visible";
    this.ready = true;
    this.text("[data-model-loading]", "Drag to orbit · Scroll to zoom");
    this.play(this.currentClip);
  };

  private syncClips(): void {
    const available = this.ready && this.currentStage !== "reference" ? this.model.availableAnimations : [];
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-model-clip]")) {
      button.disabled = !available.includes(button.dataset.modelClip!);
      button.setAttribute("aria-pressed", String(!button.disabled && button.dataset.modelClip === this.currentClip));
    }
    const pause = this.query<HTMLButtonElement>("[data-model-pause]");
    pause.disabled = available.length === 0;
    pause.textContent = this.model.paused ? "Play" : "Pause";
  }

  private play(clip: string): void {
    if (this.currentStage === "reference") return;
    if (this.model.availableAnimations.includes(clip)) {
      // Match the rendered cover's six-second playback of the four-second cheer.
      this.model.timeScale = clip === "cheer" ? 2 / 3 : 1;
      this.currentClip = clip; this.model.animationName = clip; this.model.play();
    }
    this.syncClips();
  }

  private showEvidence = (): void => {
    const path = this.query<HTMLSelectElement>("[data-model-evidence]").value;
    const image = this.query<HTMLImageElement>("[data-model-evidence-image]");
    image.hidden = !path;
    if (path) {
      image.src = this.asset(path);
      image.alt = this.data?.images.find((item) => item.url === path)?.label || "Recorded rig evidence";
    }
  };

  private click = (event: MouseEvent): void => {
    const target = event.target instanceof Element ? event.target : null;
    const stage = target?.closest<HTMLElement>("[data-model-stage]")?.dataset.modelStage;
    if (stage) this.selectStage(stage);
    const clip = target?.closest<HTMLElement>("[data-model-clip]")?.dataset.modelClip;
    if (clip && this.ready) this.play(clip);
    if (target?.closest("[data-model-pause]") && this.ready) {
      if (this.model.paused) this.model.play(); else this.model.pause();
      this.syncClips();
    }
    if (target?.closest("[data-model-reset]")) {
      this.model.cameraOrbit = "25deg 78deg auto";
      this.model.cameraTarget = "auto auto auto";
      this.model.fieldOfView = "auto";
      this.model.jumpCameraToGoal?.();
    }
  };
}
