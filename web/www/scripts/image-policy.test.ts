import { expect, test } from "bun:test";
import type { NextConfig } from "next";
import config from "../next.config";
import { assertImageConfig, auditImageSource, checkImagePolicy } from "./image-policy";

test("current website has no optimizer or proxy asset route", async () => {
  await expect(checkImagePolicy()).resolves.toBeUndefined();
});

test("next/image requires explicit literal unoptimized, including aliased imports", () => {
  const prelude = 'import Picture from "next/image";';
  for (const props of ["", "unoptimized={false}", "unoptimized={enabled}", 'unoptimized="true"', "{...props} unoptimized", "unoptimized loader={relay}"]) {
    expect(auditImageSource(`${prelude}<Picture src="https://cdn.example/img.webp" ${props} />`)).not.toHaveLength(0);
  }
  for (const props of ["unoptimized", "unoptimized={true}"]) expect(auditImageSource(`${prelude}<Picture src={cdnUrl} ${props} />`)).toEqual([]);
  expect(auditImageSource('import { default as Photo } from "next/image"; <Photo src={url} unoptimized />')).toEqual([]);
  expect(auditImageSource('import Image from "next/legacy/image"; <Image src={url} />')).not.toHaveLength(0);
  expect(auditImageSource('<img src={cdnUrl} />')).toEqual([]);
  expect(auditImageSource('const code = \'<Image src="https://example.com/a.png" />\';')).toEqual([]);
});

test("hidden image wrappers, helper imports and hand-authored optimizer URLs fail", () => {
  for (const source of [
    'import Image from "next/image"; const Wrapper = Image;',
    'import Image from "next/image"; React.createElement(Image, props);',
    'import * as Image from "next/image"; <Image.default src={url} />;',
    'export { default } from "next/image";',
    'const Image = require("next/image");',
    'const Image = await import("next/image");',
    'import { getImageProps } from "next/image";',
    '<img src="/fx/_next/image?url=https%3A%2F%2Fcdn.example%2Fa.png" />',
    '<img src={`/_next/image?url=${cdnUrl}`} />',
  ]) expect(auditImageSource(source)).not.toHaveLength(0);
});

test("optimizer configuration cannot allow a CDN, wildcard, custom loader or proxy", () => {
  for (const patch of [
    { unoptimized: false },
    { remotePatterns: [{ protocol: "https", hostname: "assets.example.com" }] },
    { remotePatterns: [{ hostname: "**" }] },
    { remotePatterns: [{ hostname: "**.example.com" }] },
    { remotePatterns: [new URL("https://assets.example.com/**")] },
    { domains: ["assets.example.com"] },
    { loader: "custom", loaderFile: "./relay.ts" },
    { path: "/fx/asset-proxy" },
  ]) expect(() => assertImageConfig({ ...config, images: { ...config.images, ...patch } } as NextConfig)).toThrow();
  expect(() => assertImageConfig({ ...config, images: { unoptimized: true } })).toThrow();
  expect(() => assertImageConfig({ ...config, output: "standalone" })).toThrow();
  expect(() => assertImageConfig({ ...config, rewrites: async () => [] })).toThrow();
});

test("Next emits the original CDN URL and rejects remote optimization if enabled accidentally", async () => {
  // Next skips host checking under NODE_ENV=test; isolate its real development path.
  const child = Bun.spawn([process.execPath, "--no-env-file", "-e", `
    import { getImgProps } from "next/dist/shared/lib/get-img-props.js";
    import loaderModule from "next/dist/shared/lib/image-loader.js";
    import { imageConfigDefault } from "next/dist/shared/lib/image-config.js";
    import { ImageOptimizerCache } from "next/dist/server/image-optimizer.js";
    import config from "./next.config.ts";
    const defaultLoader = typeof loaderModule === "function" ? loaderModule : loaderModule.default;
    const src = "https://assets.example.com/showcases/immutable/cover.webp";
    const state = { defaultLoader, imgConf: { ...imageConfigDefault, ...config.images, allSizes: [...imageConfigDefault.deviceSizes, ...imageConfigDefault.imageSizes].sort((a,b) => a-b) } };
    const direct = getImgProps({ src, alt: "Example", width: 960, height: 600, unoptimized: true }, state).props;
    let rejected = false;
    try { getImgProps({ src, alt: "Example", width: 960, height: 600 }, { ...state, imgConf: { ...state.imgConf, unoptimized: false } }); }
    catch (error) { if (!error.message.includes("not configured")) throw error; rejected = true; }
    const production = ImageOptimizerCache.validateParams({ headers: {} }, { url: src, w: "960", q: "75" }, config, false);
    console.log(JSON.stringify({ src: direct.src, hasSrcSet: direct.srcSet !== undefined, rejected, productionError: production.errorMessage }));
  `], { cwd: new URL("../", import.meta.url).pathname, env: { ...process.env, NODE_ENV: "development" }, stdout: "pipe", stderr: "pipe" });
  const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  expect(error).toBe("");
  expect(code).toBe(0);
  expect(JSON.parse(output)).toEqual({ src: "https://assets.example.com/showcases/immutable/cover.webp", hasSrcSet: false, rejected: true, productionError: '"url" parameter is not allowed' });
});
