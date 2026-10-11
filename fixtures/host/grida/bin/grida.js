#!/usr/bin/env node
// A stand-in for the npm `grida` command (gridaco/grida, packages/grida-cli), reduced to what FX
// owns of it: the `fx` verb (spec/distribution.md §3). The real launcher carries the branch below
// as it is; everything else it does is not here.
const [verb, ...rest] = process.argv.slice(2);
if (verb === "fx") {
  // Loaded only for `grida fx`, so no other grida command pays for it.
  const { HOST_CONTRACT, launch } = await import("@grida/fx/cli");
  if (HOST_CONTRACT !== 1) {
    process.stderr.write(
      `grida fx: @grida/fx speaks host contract ${HOST_CONTRACT}, not 1; reinstall grida\n`,
    );
    process.exit(1);
  }
  launch(rest, { invoker: "grida" });
} else {
  process.stderr.write("grida (stand-in): only `grida fx` is here\n");
  process.exit(2);
}
