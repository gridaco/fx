#!/usr/bin/env bun
/** Original, deterministic GLB 2 fixtures for viewer tests. No downloaded artwork or tools.
 * Import modelFixture() for bytes; `bun tools/make_model_fixture.mjs out.glb` writes one.
 */
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(kind, bytes) {
  const name = Buffer.from(kind), out = Buffer.alloc(bytes.length + 12);
  out.writeUInt32BE(bytes.length); name.copy(out, 4); bytes.copy(out, 8);
  out.writeUInt32BE(crc32(Buffer.concat([name, bytes])), bytes.length + 8);
  return out;
}
/** A 4×4 checker with unmistakably different original red, yellow, blue and green texels. */
function checker() {
  const header = Buffer.alloc(13); header.writeUInt32BE(4); header.writeUInt32BE(4, 4); header[8] = 8; header[9] = 2;
  const pixels = [[225, 40, 45], [245, 195, 30], [35, 80, 225], [30, 180, 80]];
  const rows = Buffer.alloc(4 * 13);
  for (let y = 0; y < 4; y++) for (let x = 0; x < 4; x++) {
    const color = pixels[(x < 2 ? 0 : 1) + (y < 2 ? 0 : 2)];
    rows.set(color, y * 13 + 1 + x * 3);
  }
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(rows)), chunk("IEND", Buffer.alloc(0))]);
}

/**
 * A two-section rectangular column, checker-textured with authored PBR factors. Its two-joint
 * skin bends through weighted vertices in two clips. Variants exercise distinct viewer limits.
 */
export function modelFixture({ animated = true, skinned = true, motionOnly = false, external = false, dataUris = false, requiredExtension = null, clipNames = ["Bend", "Nod"], scale = [1, 1, 1] } = {}) {
  const positions = [], normals = [], texcoords = [], joints = [], weights = [], indices = [];
  function face(vertices, normal) {
    const base = positions.length / 3;
    for (let index = 0; index < 4; index++) {
      const [x, y, z] = vertices[index]; positions.push(x, y, z); normals.push(...normal);
      texcoords.push(...[[0, 0], [1, 0], [1, 1], [0, 1]][index]);
      const upper = Math.max(0, Math.min(1, (y - .75) / 1.5));
      joints.push(0, 1, 0, 0); weights.push(1 - upper, upper, 0, 0);
    }
    indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
  }
  for (let section = 0; section < 2; section++) {
    const low = section * 1.5, high = low + 1.5, x = .6, z = .35;
    face([[-x, low, z], [x, low, z], [x, high, z], [-x, high, z]], [0, 0, 1]);
    face([[x, low, -z], [-x, low, -z], [-x, high, -z], [x, high, -z]], [0, 0, -1]);
    face([[x, low, z], [x, low, -z], [x, high, -z], [x, high, z]], [1, 0, 0]);
    face([[-x, low, -z], [-x, low, z], [-x, high, z], [-x, high, -z]], [-1, 0, 0]);
    face([[-x, high, z], [x, high, z], [x, high, -z], [-x, high, -z]], [0, 1, 0]);
    face([[-x, low, -z], [x, low, -z], [x, low, z], [-x, low, z]], [0, -1, 0]);
  }
  const parts = [], views = [], accessors = [];
  let length = 0;
  function bufferView(bytes) {
    const padding = (4 - length % 4) % 4;
    if (padding) { parts.push(Buffer.alloc(padding)); length += padding; }
    const index = views.length;
    views.push({ buffer: 0, byteOffset: length, byteLength: bytes.byteLength });
    parts.push(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength)); length += bytes.byteLength;
    return index;
  }
  function accessor(values, type, width, extra = {}) {
    const index = accessors.length;
    accessors.push({ bufferView: bufferView(values), componentType: values instanceof Float32Array ? 5126 : 5123, count: values.length / width, type, ...extra });
    return index;
  }
  const attributes = {
    POSITION: accessor(new Float32Array(positions), "VEC3", 3, { min: [-.6, 0, -.35], max: [.6, 3, .35] }),
    NORMAL: accessor(new Float32Array(normals), "VEC3", 3),
    TEXCOORD_0: accessor(new Float32Array(texcoords), "VEC2", 2),
  };
  if (skinned) {
    attributes.JOINTS_0 = accessor(new Uint16Array(joints), "VEC4", 4);
    attributes.WEIGHTS_0 = accessor(new Float32Array(weights), "VEC4", 4);
  }
  const triangles = accessor(new Uint16Array(indices), "SCALAR", 1);
  const identity = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  const upperBind = [...identity]; upperBind[13] = -1.5;
  const matrices = accessor(new Float32Array([...identity, ...upperBind]), "MAT4", 16);
  const imageView = bufferView(checker());
  const document = {
    asset: { version: "2.0", generator: "FX original synthetic model fixture" },
    scene: 0, scenes: [{ nodes: motionOnly ? [1] : [0, 1] }],
    nodes: [{ name: "Checker column", mesh: 0, ...(scale.some(value => value !== 1) ? { scale } : {}), ...(skinned ? { skin: 0 } : {}) }, { name: "Root", children: [2] }, { name: "Upper joint", translation: [0, 1.5, 0] }],
    meshes: [{ primitives: [{ attributes, indices: triangles, material: 0 }] }],
    materials: [{ name: "Original checker PBR", pbrMetallicRoughness: { baseColorTexture: { index: 0 }, metallicFactor: .15, roughnessFactor: .7 }, doubleSided: true }],
    textures: [{ sampler: 0, source: 0 }], samplers: [{ magFilter: 9728, minFilter: 9728, wrapS: 10497, wrapT: 10497 }],
    images: external ? [{ uri: "https://untrusted.invalid/texture.png" }] : dataUris ? [{ uri: `data:image/png;base64,${checker().toString("base64")}` }] : [{ bufferView: imageView, mimeType: "image/png" }],
    ...(skinned ? { skins: [{ inverseBindMatrices: matrices, skeleton: 1, joints: [1, 2] }] } : {}),
    bufferViews: views, accessors,
  };
  if (animated) {
    document.animations = clipNames.map((name, axis) => {
      const duration = axis === 0 ? 2 : 3;
      const input = accessor(new Float32Array([0, duration / 4, duration / 2, duration * .75, duration]), "SCALAR", 1, { min: [0], max: [duration] });
      const rotations = [0, Math.PI / 3, 0, -Math.PI / 3, 0].flatMap(angle => axis === 0 ? [0, 0, Math.sin(angle / 2), Math.cos(angle / 2)] : [Math.sin(angle / 2), 0, 0, Math.cos(angle / 2)]);
      const output = accessor(new Float32Array(rotations), "VEC4", 4);
      return { name, samplers: [{ input, output, interpolation: "LINEAR" }], channels: [{ sampler: 0, target: { node: 2, path: "rotation" } }] };
    });
  }
  if (requiredExtension) {
    document.extensionsUsed = [requiredExtension]; document.extensionsRequired = [requiredExtension];
    document.extensions = { [requiredExtension]: {} };
  }
  const binary = Buffer.concat(parts), binarySize = Math.ceil(binary.length / 4) * 4;
  document.buffers = [{ byteLength: binary.length, ...(dataUris ? { uri: `data:application/octet-stream;base64,${binary.toString("base64")}` } : {}) }];
  const json = Buffer.from(JSON.stringify(document)), jsonSize = Math.ceil(json.length / 4) * 4;
  const out = Buffer.alloc(12 + 8 + jsonSize + (dataUris ? 0 : 8 + binarySize));
  out.writeUInt32LE(0x46546c67); out.writeUInt32LE(2, 4); out.writeUInt32LE(out.length, 8);
  out.writeUInt32LE(jsonSize, 12); out.writeUInt32LE(0x4e4f534a, 16);
  out.fill(0x20, 20, 20 + jsonSize); json.copy(out, 20);
  if (!dataUris) {
    out.writeUInt32LE(binarySize, 20 + jsonSize); out.writeUInt32LE(0x004e4942, 24 + jsonSize); binary.copy(out, 28 + jsonSize);
  }
  return new Uint8Array(out);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (!process.argv[2]) throw new Error("Usage: bun tools/make_model_fixture.mjs OUTPUT.glb");
  writeFileSync(process.argv[2], modelFixture());
}
