// A CommonJS module: FX hosts ES modules only.

const { node } = require("@grida/fx");

exports.common = node({ name: "common", outputs: { text: "text" } }, (ctx) => ({ text: ctx.out.text("common") }));
