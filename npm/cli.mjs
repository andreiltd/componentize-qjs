import { runCli } from "./index.js";

const args = process.argv.slice(2);
const success = await runCli(args);

if (!success) process.exitCode = 1;
