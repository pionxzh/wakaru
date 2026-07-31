#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import {
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { transform } from 'esbuild';

const root = dirname(fileURLToPath(import.meta.url));
const targetDirectory = join(root, 'target');
const outputPath = join(root, 'dist', 'angular-19.js');
const check = process.argv.slice(2).includes('--check');

const result = spawnSync(
  process.execPath,
  [
    join(root, 'node_modules', '@angular', 'compiler-cli', 'bundles', 'src', 'bin', 'ngc.js'),
    '-p',
    'tsconfig.json',
  ],
  {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 20 * 1024 * 1024,
  },
);
if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  throw new Error(result.stderr || result.stdout || `command exited ${result.status}`);
}

const compiled = readFileSync(join(targetDirectory, 'compat.component.js'), 'utf8');
const transformed = await transform(compiled, {
  charset: 'utf8',
  define: { ngDevMode: 'false' },
  format: 'esm',
  legalComments: 'none',
  minifySyntax: true,
  target: 'es2022',
  treeShaking: true,
});
const generated = `/* Generated with Angular 19.2.25; see generate.mjs. */\n${transformed.code}`;

if (
  generated.includes('ɵsetClassMetadata') ||
  generated.includes('template: `') ||
  generated.includes('<button') ||
  !generated.includes('ɵɵdefineComponent') ||
  !generated.includes('compat-card')
) {
  throw new Error('Angular compatibility output is not production full-AOT code');
}

if (check) {
  if (readFileSync(outputPath, 'utf8') !== generated) {
    throw new Error('dist/angular-19.js is stale; run npm run generate');
  }
} else {
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, generated);
}

rmSync(targetDirectory, { recursive: true, force: true });
