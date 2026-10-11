import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import type { NextConfig } from "next";
import { parse } from "@babel/parser";
import * as t from "@babel/types";
import config from "../next.config";

const wwwRoot = fileURLToPath(new URL("../", import.meta.url));
const imageModules = new Set(["next/image", "next/legacy/image"]);

/** No external host (including wildcard matches) may enter the optimizer. */
export function assertImageConfig(value: NextConfig): void {
  const images = value.images;
  if (value.output !== "export" || images?.unoptimized !== true) throw new Error("Keep static export and images.unoptimized: true for direct asset delivery.");
  if (!Array.isArray(images.remotePatterns) || images.remotePatterns.length || (images.domains?.length ?? 0)) throw new Error("Image remotePatterns and domains must be empty, including wildcard allowances.");
  if ((images.loader && images.loader !== "default") || images.loaderFile || (images.path && images.path !== "/_next/image")) throw new Error("Custom image loaders and optimizer paths are forbidden; use direct CDN URLs.");
  if (value.rewrites) throw new Error("Website rewrites can proxy CDN bytes; keep assets direct.");
}

/** Deliberately conservative: prefer img over next/image wrappers or opaque props. */
export function auditImageSource(source: string, filename = "image.tsx"): string[] {
  const file = parse(source, { sourceType: "module", plugins: ["typescript", "jsx"] });
  const bindings = new Set<string>(), declarationNames = new Set<t.Node>(), errors: string[] = [];
  const report = (node: t.Node, message: string) => errors.push(`${filename}:${node.loc?.start.line ?? 1}: ${message}`);
  for (const statement of file.program.body) {
    if (t.isImportDeclaration(statement) && imageModules.has(statement.source.value)) {
      if (statement.importKind === "type") continue;
      for (const item of statement.specifiers) {
        if (t.isImportDefaultSpecifier(item)) { bindings.add(item.local.name); declarationNames.add(item.local); }
        else if (t.isImportNamespaceSpecifier(item)) report(statement, "Use a direct default next/image import with explicit unoptimized, or prefer img.");
        else if (item.importKind !== "type") {
          if (!t.isIdentifier(item.imported, { name: "default" })) report(item, "next/image helper imports hide delivery intent; prefer img.");
          else { bindings.add(item.local.name); declarationNames.add(item.local); declarationNames.add(item.imported); }
        }
      }
    }
    if ((t.isExportNamedDeclaration(statement) || t.isExportAllDeclaration(statement)) && statement.source && imageModules.has(statement.source.value)) report(statement, "Do not re-export next/image behind a wrapper.");
  }
  function visit(node: t.Node, parent: t.Node | null = null) {
    if ((t.isImportExpression(node) && t.isStringLiteral(node.source) && imageModules.has(node.source.value))
      || (t.isCallExpression(node) && t.isIdentifier(node.callee, { name: "require" }) && t.isStringLiteral(node.arguments[0]) && imageModules.has(node.arguments[0].value))) report(node, "Dynamic next/image loading hides delivery intent; use img or a direct default import.");
    if (t.isJSXOpeningElement(node)) {
      if (t.isJSXIdentifier(node.name) && bindings.has(node.name.name)) {
        const props = node.attributes;
        const explicit = props.filter(t.isJSXAttribute).filter(prop => t.isJSXIdentifier(prop.name, { name: "unoptimized" }));
        if (explicit.length !== 1 || (explicit[0].value && !(t.isJSXExpressionContainer(explicit[0].value) && t.isBooleanLiteral(explicit[0].value.expression, { value: true })))) report(node, "next/image requires explicit literal unoptimized (or unoptimized={true}); prefer img.");
        if (props.some(t.isJSXSpreadAttribute)) report(node, "next/image prop spreads can override delivery intent; keep props explicit or use img.");
        if (props.some(prop => t.isJSXAttribute(prop) && t.isJSXIdentifier(prop.name, { name: "loader" }))) report(node, "Custom image loaders are forbidden; use the CDN URL directly.");
      }
    }
    if ((t.isIdentifier(node) || t.isJSXIdentifier(node)) && bindings.has(node.name) && !declarationNames.has(node)) {
      if (!((t.isJSXOpeningElement(parent) || t.isJSXClosingElement(parent)) && parent.name === node)) report(node, "Do not alias or wrap next/image; delivery intent must be visible at its JSX use.");
    }
    if ((t.isStringLiteral(node) && /\/_next\/image(?:[/?#]|$)/.test(node.value)) || (t.isTemplateElement(node) && /\/_next\/image(?:[/?#]|$)/.test(node.value.raw))) report(node, "Do not construct a Next image optimizer URL; reference the CDN directly.");
    for (const key of t.VISITOR_KEYS[node.type] ?? []) {
      const child = (node as unknown as Record<string, t.Node | t.Node[] | null>)[key];
      if (Array.isArray(child)) for (const item of child) { if (item) visit(item, node); }
      else if (child) visit(child, node);
    }
  }
  visit(file);
  return errors;
}

async function sourceFiles(root: string): Promise<string[]> {
  const files: string[] = [];
  for (const item of await readdir(root, { withFileTypes: true })) {
    const path = join(root, item.name);
    if (item.isDirectory()) files.push(...await sourceFiles(path));
    else if (/\.[cm]?[jt]sx?$/.test(item.name)) files.push(path);
  }
  return files;
}

export async function checkImagePolicy(): Promise<void> {
  assertImageConfig(config);
  const files = await sourceFiles(join(wwwRoot, "src"));
  const errors = (await Promise.all(files.map(async path => auditImageSource(await readFile(path, "utf8"), relative(wwwRoot, path))))).flat();
  for (const path of [...files, ...(await readdir(wwwRoot)).map(name => join(wwwRoot, name))]) {
    if (/\/(?:proxy|middleware)\.[cm]?[jt]sx?$/.test(path)) errors.push(`${relative(wwwRoot, path)}: Middleware/proxies must not relay external assets.`);
  }
  const vercel = await readFile(join(wwwRoot, "vercel.json"), "utf8").catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
  if (vercel) {
    const routing = JSON.parse(vercel);
    if (routing.rewrites?.length || routing.routes?.length) errors.push("vercel.json: Proxy routing requires revisiting the direct asset policy.");
  }
  if (errors.length) throw new Error(`Direct asset policy failed:\n${errors.join("\n")}`);
}

if (import.meta.main) { await checkImagePolicy(); console.log("Direct CDN image policy passed."); }
