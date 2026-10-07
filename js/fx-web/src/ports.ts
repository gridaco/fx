/** Port declarations and resolved bindings are emitted by FX, never inferred by the viewer. */
export interface NodePorts {
  inputs: Record<string, string>;
  outputs: Record<string, string>;
  params: Record<string, Record<string, unknown>>;
}

export interface PortBinding {
  source: string;
  source_port: string;
  target_port: string;
  source_kind: "output" | "fact";
}

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const name = (value: unknown): value is string => typeof value === "string" && value.length > 0;
const portName = (value: unknown): value is string => typeof value === "string" && /^[a-z][a-z0-9_]*$/.test(value);
const notation = /^(?:image|audio|video|model|text|json|annotations|file)(?:\/[a-z0-9.+-]+)?(?:\[\]|\{\})?\??$/;

export function isNodePorts(value: unknown): value is NodePorts {
  return record(value) && Object.keys(value).length === 3
    && record(value.inputs) && record(value.outputs) && record(value.params)
    && [value.inputs, value.outputs].every((ports) => Object.entries(ports).every(([key, type]) => portName(key) && typeof type === "string" && notation.test(type)))
    && Object.entries(value.params).every(([key, schema]) => portName(key) && record(schema));
}

export function isPortBindings(value: unknown): value is PortBinding[] {
  return Array.isArray(value) && value.every((binding) => record(binding)
    && Object.keys(binding).length === 4
    && name(binding.source) && portName(binding.target_port)
    && ((binding.source_kind === "output" && portName(binding.source_port))
      || (binding.source_kind === "fact" && typeof binding.source_port === "string")))
    && new Set(value.map((binding) => JSON.stringify([binding.source, binding.source_port, binding.target_port, binding.source_kind]))).size === value.length;
}

export function displayPortName(name: string) { return name === "" ? '""' : name; }

/** A display label only; schema evaluation remains the engine's responsibility. */
export function parameterType(schema: unknown): string {
  if (!record(schema)) return "value";
  if (typeof schema.type === "string") return schema.type;
  if (Array.isArray(schema.type) && schema.type.every((item) => typeof item === "string")) return schema.type.join(" | ");
  if (Array.isArray(schema.enum)) return "enum";
  return "value";
}
