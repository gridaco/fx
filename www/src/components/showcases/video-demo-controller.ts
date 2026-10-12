import { parseVideoDemo, safePath, type VideoDemoManifest, type VideoDemoMedia, type VideoDemoSlug } from "../../lib/showcases/contracts";
import { loadShowcaseJson } from "../../lib/showcases/load";

type MotionPreference = Pick<MediaQueryList, "matches" | "addEventListener" | "removeEventListener">;
type ObserverFactory = (callback: IntersectionObserverCallback) => Pick<IntersectionObserver, "observe" | "disconnect">;

/** Native controls own playback. This class owns deferred loading and lifecycle cleanup. */
export class VideoDemoPlayback {
  private attached = false;
  private disposed = false;
  private visible = false;
  private explicit = false;
  private readonly observer;
  private readonly preferenceChanged = () => {
    this.explicit = false;
    this.video.pause();
    if (this.motion.matches) this.clearSource();
    else if (this.visible) this.attach();
    this.changed(this.attached);
  };

  constructor(
    private readonly video: HTMLVideoElement,
    private readonly source: string,
    private readonly changed: (attached: boolean) => void = () => {},
    private readonly motion: MotionPreference = matchMedia("(prefers-reduced-motion: reduce)"),
    observe: ObserverFactory = callback => new IntersectionObserver(callback),
  ) {
    this.observer = observe(entries => {
      this.visible = entries.some(entry => entry.target === video && entry.isIntersecting);
      if (this.disposed) return;
      if (!this.visible) this.video.pause();
      else if (!this.motion.matches || this.explicit) this.attach();
    });
    motion.addEventListener("change", this.preferenceChanged);
    this.observer.observe(video);
  }

  /** Reduced motion suppresses automatic loading, while deliberate playback remains available. */
  play() {
    if (this.disposed) return;
    this.explicit = true;
    this.attach();
    void this.video.play().catch(() => {});
  }

  private attach() {
    if (this.attached || this.disposed) return;
    this.video.src = this.source;
    this.video.preload = "metadata";
    this.attached = true;
    this.changed(true);
  }

  private clearSource() {
    if (!this.attached) return;
    this.video.removeAttribute("src");
    this.video.load();
    this.attached = false;
  }

  dispose() {
    this.disposed = true;
    this.observer.disconnect();
    this.motion.removeEventListener("change", this.preferenceChanged);
    this.video.pause();
    this.clearSource();
  }
}

type ManifestLoader = (signal: AbortSignal) => Promise<VideoDemoManifest>;

/** The website projects pinned media only; it never reconstructs or executes a video workflow. */
export class VideoDemoController {
  private readonly events = new AbortController();
  private readonly players: VideoDemoPlayback[] = [];
  private disposed = false;

  constructor(private readonly root: HTMLElement, private readonly slug: VideoDemoSlug, private readonly assetBase: string, load?: ManifestLoader) {
    const loader = load ?? (signal => loadShowcaseJson(slug, assetBase, "manifest.json", parseVideoDemo, signal));
    void loader(this.events.signal).then(data => {
      if (!this.disposed) this.render(data);
    }).catch(error => {
      if (!this.disposed) this.status(error instanceof Error ? error.message : "This video example could not be loaded.");
    });
  }

  private status(message: string) {
    const status = this.root.querySelector<HTMLElement>("[data-video-status]");
    if (status) status.textContent = message;
  }

  private element<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string): HTMLElementTagNameMap[K] {
    const element = this.root.ownerDocument.createElement(tag);
    if (text !== undefined) element.textContent = text;
    return element;
  }

  private asset(file: string) { return `${this.assetBase.replace(/\/+$/, "")}/${safePath(file)}`; }

  private media(item: VideoDemoMedia, result?: VideoDemoManifest["result"]) {
    const figure = this.element("figure"), frame = this.element("div");
    figure.dataset.videoMedia = item.media_type.startsWith("video/") || item.media_type.startsWith("image/") ? "visual" : "document";
    frame.dataset.videoFrame = "";
    if (result) {
      frame.dataset.final = "";
      frame.style.aspectRatio = `${result.width} / ${result.height}`;
    }
    if (item.media_type.startsWith("video/")) {
      const video = this.element("video");
      video.controls = false;
      video.playsInline = true;
      video.preload = "none";
      video.poster = this.asset(item.poster!);
      video.setAttribute("aria-label", item.label);
      video.setAttribute("aria-hidden", "true");
      // Comparison clips are silent by default; the finished result retains its authored audio.
      video.muted = !result;
      if (result) { video.width = result.width; video.height = result.height; }
      const play = this.element("button", "Play video");
      play.type = "button";
      play.dataset.videoPlay = "";
      play.setAttribute("aria-label", `Play ${item.label}`);
      frame.append(video, play);
      const player = new VideoDemoPlayback(video, this.asset(item.file), attached => {
        play.hidden = attached;
        video.controls = attached;
        if (attached) video.removeAttribute("aria-hidden");
        else video.setAttribute("aria-hidden", "true");
      });
      this.players.push(player);
      play.addEventListener("click", () => player.play(), { signal: this.events.signal });
      video.addEventListener("error", () => {
        const warning = this.element("p", `Unable to play ${item.label}. Open the recorded file below.`);
        warning.dataset.videoError = "";
        frame.append(warning);
      }, { signal: this.events.signal });
    } else if (item.media_type.startsWith("image/")) {
      const image = this.element("img");
      image.src = this.asset(item.file);
      image.alt = item.label;
      image.loading = "lazy";
      image.decoding = "async";
      frame.append(image);
    }
    const caption = this.element("figcaption"), download = this.element("a", item.label);
    download.href = this.asset(item.file);
    download.dataset.videoDownload = "";
    download.setAttribute("download", "");
    caption.append(download);
    if (frame.childElementCount) figure.append(frame);
    figure.append(caption);
    return figure;
  }

  private render(data: VideoDemoManifest) {
    const content = this.root.querySelector<HTMLElement>("[data-video-content]");
    if (!content) throw new Error("Missing video example content element.");
    const final = this.element("section");
    final.dataset.videoResult = "";
    const intro = this.element("p", data.summary);
    intro.dataset.videoSummary = "";
    const result = this.media({ ...data.result, label: "Finished clip" }, data.result);
    const timing = this.element("p", `${data.result.width} × ${data.result.height} · ${data.result.duration_seconds.toFixed(2)} s`);
    timing.dataset.videoTiming = "";
    final.append(result, timing, intro);

    const references = this.element("section"), referenceHeading = this.element("h3", "Inputs and references"), grid = this.element("div");
    grid.dataset.videoInputs = "";
    references.append(referenceHeading, grid);
    for (const input of data.inputs) {
      const figure = this.media(input), caption = figure.querySelector("figcaption")!;
      caption.prepend(this.element("span", `${input.role[0].toUpperCase()}${input.role.slice(1)} · `));
      caption.append(this.element("p", input.description));
      grid.append(figure);
    }

    const process = this.element("details"), processHeading = this.element("summary", "Recorded stage outputs"), stages = this.element("ol");
    stages.dataset.videoStages = "";
    process.append(processHeading, stages);
    for (const stage of data.stages) {
      const item = this.element("li"), heading = this.element("h4", stage.label), description = this.element("p", stage.description), outputs = this.element("div");
      outputs.dataset.videoOutputs = "";
      outputs.append(...stage.outputs.map(output => this.media(output)));
      item.append(heading, description, outputs);
      stages.append(item);
    }
    content.replaceChildren(final, references, process);
    if (data.limitations.length) {
      const scope = this.element("aside"), heading = this.element("h3", "This example's scope"), items = this.element("ul");
      items.append(...data.limitations.map(item => this.element("li", item)));
      scope.dataset.videoScope = "";
      scope.append(heading, items);
      content.append(scope);
    }
    this.root.dataset.videoKind = this.slug;
    this.status("");
  }

  dispose() {
    this.disposed = true;
    this.events.abort();
    this.players.forEach(player => player.dispose());
    this.players.length = 0;
  }
}
