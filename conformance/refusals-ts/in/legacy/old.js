// A .js module that Node loads as CommonJS, since legacy/package.json says so: FX hosts ES
// modules only, and refuses it before its code runs, which would leave ran.txt behind.

require("node:fs").writeFileSync("ran.txt", "legacy/old.js ran\n");
const { node } = require("@grida/fx");

exports.old = node({ name: "old", outputs: { text: "text" } }, (ctx) => ({ text: ctx.out.text("old") }));
