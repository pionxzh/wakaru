#!/usr/bin/env bun

import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const directory = dirname(fileURLToPath(import.meta.url));
const asset = join(directory, "asset.bin");
const executable = join(directory, "fixture-app");
writeFileSync(
  asset,
  Buffer.from([0x00, 0xff, 0x42, 0x75, 0x6e, 0x0a, 0x80, 0x41, 0x53, 0x53, 0x45, 0x54]),
);
const result = Bun.spawnSync({
  cmd: [
    process.execPath,
    "build",
    "--compile",
    join(directory, "entry.ts"),
    "--outfile",
    executable,
  ],
  stdout: "inherit",
  stderr: "inherit",
});
if (result.exitCode !== 0) {
  process.exit(result.exitCode);
}

const bytes = readFileSync(executable);
const trailer = Buffer.from("\n---- Bun! ----\n");
const trailerStart = bytes.lastIndexOf(trailer);
if (trailerStart < 32) {
  throw new Error("compiled fixture does not contain a Bun standalone trailer");
}
const byteCount = Number(bytes.readBigUInt64LE(trailerStart - 32));
const dataStart = trailerStart - 32 - byteCount;
writeFileSync(
  join(directory, "standalone.bin"),
  bytes.subarray(dataStart, trailerStart + trailer.length),
);
rmSync(executable);
