/**
 * Suffix tables: the kind of a file by its suffix (`spec/identity.md` §4), as `ctx.out.file` and
 * stand-in answers give it, and the suffixes of the modules the TypeScript node host loads.
 */

/** A file's kind by its suffix, compared case-insensitively (`spec/identity.md` §4). */
export const KINDS: Readonly<Record<string, string>> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".gif": "image/gif",
  ".md": "text/markdown",
  ".txt": "text/plain",
  ".json": "json",
  ".yaml": "text/yaml",
  ".yml": "text/yaml",
  ".toml": "text/toml",
  ".html": "text/html",
  ".wav": "audio/wav",
  ".mp3": "audio/mpeg",
  ".ogg": "audio/ogg",
  ".mp4": "video/mp4",
  ".webm": "video/webm",
  ".mkv": "video/x-matroska",
  ".glb": "model/gltf-binary",
  ".gltf": "model/gltf+json",
  ".fbx": "model/fbx",
  ".zip": "file/zip",
};

/** The suffixes of the ES modules the TypeScript node host loads (`.tsx`, `.cts` and `.cjs` are
 * refused by the engine before any host sees them). */
export const MODULE_SUFFIXES: readonly string[] = [".ts", ".mts", ".js", ".mjs"];

/** A path's last suffix, lower-cased: `.png` for `a/b.PNG`; empty for `.bashrc` or `a.`. */
export function suffixOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) {
    return "";
  }
  return name.slice(dot).toLowerCase();
}

/** The kind of a file by its suffix; `file` when the table has none (only the last suffix counts:
 * `a.tar.gz` is `file`). */
export function kindOf(path: string): string {
  return KINDS[suffixOf(path)] ?? "file";
}

/** A kind's family: `image` for `image/png`. */
export function familyOf(kind: string): string {
  const slash = kind.indexOf("/");
  return slash < 0 ? kind : kind.slice(0, slash);
}

/** Whether the TypeScript node host loads a file with this name as a module. Suffixes are compared
 * exactly, as the engine compares them (`X.TS` is no module), and a declaration file (`x.d.ts`,
 * `x.d.mts`) is never one. */
export function isModuleFile(path: string): boolean {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || !MODULE_SUFFIXES.includes(name.slice(dot))) {
    return false;
  }
  return !/^.+\.d\.m?ts$/.test(name);
}
