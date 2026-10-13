// A workspace package, linked as node_modules/@acme/shapes and declared in fx.yaml's sources.

export function outline(text: string): string {
  return `(${text})`;
}
