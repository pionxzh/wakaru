#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  assertPinnedTest262Corpus,
  defaultManagedTest262Root,
  readTest262Revision,
} from "./test262-corpus.mjs";
import { parseTestMetadata } from "./test262-metadata.mjs";

export function auditTest262Metadata(test262Root) {
  const root = resolve(test262Root);
  const testRoot = join(root, "test");
  if (!existsSync(testRoot)) {
    throw new Error(`missing Test262 test directory: ${testRoot}`);
  }

  const files = [];
  collectTestFiles(testRoot, files);
  const errors = [];
  let fixtures = 0;
  for (const file of files.sort()) {
    const path = relative(root, file).split(sep).join("/");
    if (path.includes("_FIXTURE")) {
      fixtures += 1;
      continue;
    }
    try {
      parseTestMetadata(readFileSync(file, "utf8"));
    } catch (error) {
      errors.push({
        path,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  return {
    root,
    revision: readTest262Revision(root) ?? "unmanaged",
    discovered: files.length,
    fixtures,
    audited: files.length - fixtures,
    valid: files.length - fixtures - errors.length,
    errors,
  };
}

function collectTestFiles(path, files) {
  const stat = statSync(path);
  if (stat.isFile()) {
    if (extname(path) === ".js") files.push(path);
    return;
  }
  for (const entry of readdirSync(path)) {
    collectTestFiles(join(path, entry), files);
  }
}

function parseArgs(argv) {
  const options = { test262Root: defaultManagedTest262Root, json: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--test262") {
      const value = argv[++index];
      if (!value || value.startsWith("-")) {
        throw new Error("--test262 requires a directory");
      }
      options.test262Root = resolve(value);
    } else if (arg === "--json") {
      options.json = true;
    } else if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  return options;
}

function usage() {
  return `Usage:
  node scripts/correctness/test262-metadata-audit.mjs [options]

Options:
  --test262 <dir>  Test262 checkout. Default: managed pinned checkout
  --json           Print the complete machine-readable result
`;
}

function isMain() {
  return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
}

if (isMain()) {
  try {
    const options = parseArgs(process.argv.slice(2));
    if (options.help) {
      console.log(usage());
    } else {
      if (resolve(options.test262Root) === resolve(defaultManagedTest262Root)) {
        assertPinnedTest262Corpus({ root: options.test262Root });
      }
      const result = auditTest262Metadata(options.test262Root);
      if (options.json) {
        console.log(JSON.stringify(result, null, 2));
      } else {
        console.log(
          `Test262 metadata: ${result.valid}/${result.audited} valid ` +
            `(${result.fixtures} fixtures excluded, revision ${result.revision})`,
        );
        for (const item of result.errors.slice(0, 20)) {
          console.error(`${item.path}: ${item.error}`);
        }
        if (result.errors.length > 20) {
          console.error(`... ${result.errors.length - 20} more metadata errors`);
        }
      }
      process.exitCode = result.errors.length === 0 ? 0 : 1;
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
