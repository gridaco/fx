#!/usr/bin/env node
// The grida-fx command: runs the engine for this machine (src/cli.ts).
import { main } from "../dist/cli.js";

main(process.argv.slice(2));
