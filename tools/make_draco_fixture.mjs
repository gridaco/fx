/** Compress the original synthetic geometry with the installed, pinned Three.js encoder.
 * This is test-only processing: no network, provider, downloaded artwork, or committed media.
 */
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

let encoder;
async function encoderModule() {
  return encoder ??= new Promise((resolve, reject) => {
    try {
      // The upstream encoder is a classic Emscripten script, not an ES module.
      const context = { module: { exports: {} }, exports: {}, console, setTimeout, clearTimeout, atob, btoa };
      runInNewContext(readFileSync(new URL("../web/fx-web/node_modules/three/examples/jsm/libs/draco/draco_encoder.js", import.meta.url), "utf8"), context);
      // Emscripten's module is a self-resolving thenable; wrap it to avoid Promise assimilation.
      context.module.exports({ onModuleLoaded: module => resolve({ module }), onAbort: reject });
    } catch (error) { reject(error); }
  });
}

/** A self-contained KHR_draco_mesh_compression variant of a static fixture GLB. */
export async function dracoFixture(source) {
  const { module } = await encoderModule();
  const input = Buffer.from(source), jsonSize = input.readUInt32LE(12);
  const document = JSON.parse(input.toString("utf8", 20, 20 + jsonSize));
  const binary = input.subarray(28 + jsonSize);
  const primitive = document.meshes[0].primitives[0];
  const values = (index) => {
    const accessor = document.accessors[index], view = document.bufferViews[accessor.bufferView];
    const offset = (view.byteOffset || 0) + (accessor.byteOffset || 0);
    const width = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4 }[accessor.type];
    const ArrayType = accessor.componentType === 5126 ? Float32Array : Uint16Array;
    return new ArrayType(binary.buffer.slice(binary.byteOffset + offset, binary.byteOffset + offset + accessor.count * width * ArrayType.BYTES_PER_ELEMENT));
  };
  const mesh = new module.Mesh(), builder = new module.MeshBuilder(), compressor = new module.Encoder(), encoded = new module.DracoInt8Array();
  try {
    const attributes = {};
    for (const [semantic, attributeType, width] of [["POSITION", module.POSITION, 3], ["NORMAL", module.NORMAL, 3], ["TEXCOORD_0", module.TEX_COORD, 2]]) {
      const index = primitive.attributes[semantic];
      attributes[semantic] = builder.AddFloatAttributeToMesh(mesh, attributeType, document.accessors[index].count, width, values(index));
      compressor.SetAttributeQuantization(attributeType, 14);
    }
    const indices = values(primitive.indices);
    builder.AddFacesToMesh(mesh, indices.length / 3, indices);
    compressor.SetSpeedOptions(5, 5);
    const size = compressor.EncodeMeshToDracoBuffer(mesh, encoded);
    if (size <= 0) throw new Error("The synthetic Draco fixture could not be encoded.");
    const compressed = Buffer.alloc(size);
    for (let at = 0; at < size; at++) compressed[at] = encoded.GetValue(at) & 255;
    const offset = Math.ceil(binary.length / 4) * 4;
    const view = document.bufferViews.length;
    document.bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: size });
    primitive.extensions = { KHR_draco_mesh_compression: { bufferView: view, attributes } };
    document.extensionsUsed = ["KHR_draco_mesh_compression"];
    document.extensionsRequired = ["KHR_draco_mesh_compression"];
    for (const index of [...Object.values(primitive.attributes), primitive.indices]) {
      delete document.accessors[index].bufferView;
      delete document.accessors[index].byteOffset;
    }
    const outputBinary = Buffer.alloc(Math.ceil((offset + size) / 4) * 4);
    binary.copy(outputBinary); compressed.copy(outputBinary, offset);
    document.buffers[0].byteLength = outputBinary.length;
    const json = Buffer.from(JSON.stringify(document)), padded = Math.ceil(json.length / 4) * 4;
    const output = Buffer.alloc(28 + padded + outputBinary.length);
    output.writeUInt32LE(0x46546c67); output.writeUInt32LE(2, 4); output.writeUInt32LE(output.length, 8);
    output.writeUInt32LE(padded, 12); output.writeUInt32LE(0x4e4f534a, 16);
    output.fill(0x20, 20, 20 + padded); json.copy(output, 20);
    output.writeUInt32LE(outputBinary.length, 20 + padded); output.writeUInt32LE(0x004e4942, 24 + padded); outputBinary.copy(output, 28 + padded);
    return new Uint8Array(output);
  } finally {
    for (const item of [encoded, compressor, builder, mesh]) module.destroy(item);
  }
}
