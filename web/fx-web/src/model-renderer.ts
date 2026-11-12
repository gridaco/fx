/** The renderer lives in a separate chunk, imported only when a visible model needs a preview. */
export type ModelElement = HTMLElement & {
  src: string;
  availableAnimations: string[];
  animationName: string;
  readonly paused: boolean;
  currentTime: number;
  readonly updateComplete: Promise<boolean>;
  cameraOrbit: string;
  play(): void;
  pause(): void;
  jumpCameraToGoal(): void;
  toDataURL(type?: string, encoderOptions?: number): string;
};

type RendererConstructor = CustomElementConstructor & {
  modelCacheSize: number;
  dracoDecoderLocation: string;
  ktx2TranscoderLocation: string;
  meshoptDecoderLocation: string;
};

// URL dependencies work with both Vite and webpack. Their output configuration preserves
// decoder basenames: the upstream loaders append these fixed names to their directory URL.
const draco = [
  new URL("../node_modules/three/examples/jsm/libs/draco/gltf/draco_decoder.wasm", import.meta.url),
  new URL("../node_modules/three/examples/jsm/libs/draco/gltf/draco_wasm_wrapper.js", import.meta.url),
  new URL("../node_modules/three/examples/jsm/libs/draco/gltf/draco_decoder.js", import.meta.url),
];
const basis = [
  new URL("../node_modules/three/examples/jsm/libs/basis/basis_transcoder.wasm", import.meta.url),
  new URL("../node_modules/three/examples/jsm/libs/basis/basis_transcoder.js", import.meta.url),
];
const meshoptReady = new URL("./model-meshopt-ready.js", import.meta.url);

function directory(files: URL[]): string {
  const base = new URL("./", files[0]).href;
  if (files.some(file => new URL("./", file).href !== base)) throw new Error("3D decoder assets are unavailable.");
  return base;
}

let loading: Promise<void> | undefined;

/** Register model-viewer with local decoders before any element is created. No CDN defaults. */
export function loadModelRenderer(): Promise<void> {
  return loading ??= (async () => {
    const config = {
      dracoDecoderLocation: directory(draco),
      ktx2TranscoderLocation: directory(basis),
      meshoptDecoderLocation: meshoptReady.href,
    };
    // model-viewer's constructor reads this configuration even when registered already.
    Object.assign(globalThis as unknown as { ModelViewerElement: typeof config }, { ModelViewerElement: config });
    if (!customElements.get("model-viewer")) await import("@google/model-viewer");
    const Renderer = customElements.get("model-viewer") as RendererConstructor | undefined;
    if (!Renderer) throw new Error("The 3D renderer could not load.");
    Object.assign(Renderer, config);
    // The source controller owns lifetime. Do not retain decoded models after their
    // previews disconnect, especially when inspecting several large candidates.
    Renderer.modelCacheSize = 0;
  })().catch(() => {
    loading = undefined;
    throw new Error("The 3D renderer could not load. Download the original model to inspect it elsewhere.");
  });
}
