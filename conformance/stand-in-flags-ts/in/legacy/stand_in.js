// A stand-in file that Node would load as CommonJS, since legacy/package.json says so: refused
// before its code runs, which would leave ran.txt behind.

require("node:fs").writeFileSync("ran.txt", "legacy/stand_in.js ran\n");
exports.answer = () => ({ data: null });
