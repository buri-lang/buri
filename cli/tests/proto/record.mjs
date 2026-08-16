// Sits between the runner and the testee and copies every frame both ways, so
// that a run can be turned into checked-in vectors. Nothing here understands
// protobuf: it is a pipe with a tap on it.
import { spawn } from "node:child_process";
import { openSync, writeSync } from "node:fs";

const log = openSync(process.env.RECORD_TO, "a");
const child = spawn(process.env.JS ?? "node", [process.env.TESTEE], {
  stdio: ["pipe", "pipe", "inherit"],
});
process.stdin.on("data", (b) => {
  writeSync(log, "> " + b.toString("hex") + "\n");
  child.stdin.write(b);
});
process.stdin.on("end", () => child.stdin.end());
child.stdout.on("data", (b) => {
  writeSync(log, "< " + b.toString("hex") + "\n");
  process.stdout.write(b);
});
child.on("exit", (c) => process.exit(c ?? 0));
