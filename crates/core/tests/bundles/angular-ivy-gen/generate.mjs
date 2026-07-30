#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import {
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { transform } from 'esbuild';
import ts from 'typescript';

const root = dirname(fileURLToPath(import.meta.url));
const buildDirectory = join(root, 'target', 'angular-build', 'browser');
const constructsBuildDirectory = join(root, 'target', 'angular-constructs');
const advancedBuildDirectory = join(
  root,
  'target',
  'angular-advanced-build',
  'browser',
);
const advancedStructuralBuildDirectory = join(
  root,
  'target',
  'angular-advanced-structural-build',
  'browser',
);
const distDirectory = join(root, 'dist');

function runNode(script, args) {
  const result = spawnSync(process.execPath, [script, ...args], {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 20 * 1024 * 1024,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(result.stderr || result.stdout || `command exited ${result.status}`);
  }
}

function lowerTopLevelTemplateFunctionsToAssignments(source) {
  const sourceFile = ts.createSourceFile(
    'template-constructs.js',
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.JS,
  );
  const functions = sourceFile.statements.filter(
    (statement) =>
      ts.isFunctionDeclaration(statement) && statement.name && statement.body,
  );
  if (functions.length === 0) {
    throw new Error('expected Angular to emit top-level template functions');
  }

  const declarations = ts.factory.createVariableStatement(
    undefined,
    ts.factory.createVariableDeclarationList(
      functions.map((declaration) =>
        ts.factory.createVariableDeclaration(declaration.name.text),
      ),
      ts.NodeFlags.None,
    ),
  );
  const statements = [];
  let insertedDeclarations = false;
  for (const statement of sourceFile.statements) {
    if (!insertedDeclarations && !ts.isImportDeclaration(statement)) {
      statements.push(declarations);
      insertedDeclarations = true;
    }
    if (
      ts.isFunctionDeclaration(statement) &&
      statement.name &&
      statement.body
    ) {
      const functionExpression = ts.factory.createFunctionExpression(
        undefined,
        statement.asteriskToken,
        undefined,
        statement.typeParameters,
        statement.parameters,
        statement.type,
        statement.body,
      );
      statements.push(
        ts.factory.createExpressionStatement(
          ts.factory.createAssignment(
            ts.factory.createIdentifier(statement.name.text),
            functionExpression,
          ),
        ),
      );
    } else {
      statements.push(statement);
    }
  }
  if (!insertedDeclarations) {
    statements.push(declarations);
  }

  return ts
    .createPrinter({ newLine: ts.NewLineKind.LineFeed, removeComments: true })
    .printFile(ts.factory.updateSourceFile(sourceFile, statements));
}

rmSync(join(root, 'target'), { recursive: true, force: true });
rmSync(distDirectory, { recursive: true, force: true });

runNode(join(root, 'node_modules', '@angular', 'cli', 'bin', 'ng.js'), [
  'build',
  '--configuration=production',
]);

const generated = readdirSync(buildDirectory)
  .filter((filename) => filename.endsWith('.js'))
  .map((filename) => ({
    filename,
    size: statSync(join(buildDirectory, filename)).size,
  }));
const main = generated.find(({ filename }) => filename === 'main.js');
const chunks = generated
  .filter(({ filename }) => filename !== 'main.js')
  .sort((left, right) => right.size - left.size);

if (!main || chunks.length !== 2) {
  throw new Error(`expected main.js and two chunks, found ${JSON.stringify(generated)}`);
}

const filenameMap = new Map([
  [main.filename, 'main.js'],
  [chunks[0].filename, 'runtime.js'],
  [chunks[1].filename, 'lazy.js'],
]);

mkdirSync(distDirectory, { recursive: true });
for (const { filename } of generated) {
  let source = readFileSync(join(buildDirectory, filename), 'utf8');
  for (const [generatedName, canonicalName] of filenameMap) {
    source = source.replaceAll(`./${generatedName}`, `./${canonicalName}`);
  }
  writeFileSync(join(distDirectory, filenameMap.get(filename)), source);
}

runNode(
  join(root, 'node_modules', '@angular', 'compiler-cli', 'bundles', 'src', 'bin', 'ngc.js'),
  ['-p', 'tsconfig.constructs.json'],
);
const constructsSource = readFileSync(
  join(constructsBuildDirectory, 'template-constructs.component.js'),
  'utf8',
);
const constructsResult = await transform(constructsSource, {
  define: { ngDevMode: 'false' },
  format: 'esm',
  legalComments: 'none',
  minifySyntax: true,
  target: 'es2022',
  treeShaking: true,
});
writeFileSync(join(distDirectory, 'template-constructs.js'), constructsResult.code);
writeFileSync(
  join(distDirectory, 'template-constructs-assignment.js'),
  lowerTopLevelTemplateFunctionsToAssignments(constructsResult.code),
);

const mainSource = readFileSync(join(distDirectory, 'main.js'), 'utf8');
if (
  mainSource.includes('ɵsetClassMetadata') ||
  mainSource.includes('template: `') ||
  mainSource.includes('<article')
) {
  throw new Error('Angular output unexpectedly contains development metadata or source templates');
}
for (const selector of ['app-root', 'fixture-card']) {
  if (!mainSource.includes(selector)) {
    throw new Error(`Angular output is missing ${selector}`);
  }
}

runNode(join(root, 'node_modules', 'google-closure-compiler', 'cli.js'), [
  '--js=dist/runtime.js',
  '--js=dist/main.js',
  '--js=dist/lazy.js',
  '--js_output_file=dist/closure-simple.js',
  '--compilation_level=SIMPLE',
  '--language_in=ECMASCRIPT_NEXT',
  '--language_out=ECMASCRIPT_2022',
  '--module_resolution=NODE',
  '--warning_level=QUIET',
  '--charset=UTF-8',
]);

const closureSource = readFileSync(join(distDirectory, 'closure-simple.js'), 'utf8');
for (const selector of ['app-root', 'fixture-card', 'fixture-lazy-card']) {
  if (!closureSource.includes(selector)) {
    throw new Error(`Closure output is missing ${selector}`);
  }
}

runNode(join(root, 'node_modules', '@angular', 'cli', 'bin', 'ng.js'), [
  'build',
  '--configuration=advanced-producer',
]);

const advancedGenerated = readdirSync(advancedBuildDirectory)
  .filter((filename) => filename.endsWith('.js'))
  .sort();
if (!advancedGenerated.includes('main.js')) {
  throw new Error(
    `expected rooted ADVANCED main.js, found ${JSON.stringify(advancedGenerated)}`,
  );
}

runNode(join(root, 'node_modules', 'google-closure-compiler', 'cli.js'), [
  ...advancedGenerated.map((filename) =>
    `--js=${join(advancedBuildDirectory, filename)}`),
  `--externs=${join(root, 'closure-advanced.externs.js')}`,
  '--js_output_file=dist/closure-advanced.js',
  '--compilation_level=ADVANCED',
  '--language_in=ECMASCRIPT_NEXT',
  '--language_out=ECMASCRIPT_2022',
  '--module_resolution=NODE',
  '--dependency_mode=PRUNE',
  `--entry_point=${join(advancedBuildDirectory, 'main')}`,
  '--warning_level=QUIET',
  '--charset=UTF-8',
]);

const advancedClosureSource = readFileSync(
  join(distDirectory, 'closure-advanced.js'),
  'utf8',
);
for (const selector of ['app-root', 'fixture-card', 'fixture-lazy-card']) {
  if (!advancedClosureSource.includes(selector)) {
    throw new Error(`Rooted Closure ADVANCED output is missing ${selector}`);
  }
}
for (const contractName of [
  '__wakaruAngularDefinitions',
  '__wakaruAngularRoots',
  '__wakaruIvyRuntime',
  'ɵɵdefineComponent',
]) {
  if (!advancedClosureSource.includes(contractName)) {
    throw new Error(
      `Rooted Closure ADVANCED output is missing ${contractName}`,
    );
  }
}

runNode(join(root, 'node_modules', '@angular', 'cli', 'bin', 'ng.js'), [
  'build',
  '--configuration=advanced-structural-producer',
]);

const advancedStructuralGenerated = readdirSync(
  advancedStructuralBuildDirectory,
)
  .filter((filename) => filename.endsWith('.js'))
  .sort();
if (!advancedStructuralGenerated.includes('main.js')) {
  throw new Error(
    `expected structural ADVANCED main.js, found ${JSON.stringify(advancedStructuralGenerated)}`,
  );
}

runNode(join(root, 'node_modules', 'google-closure-compiler', 'cli.js'), [
  ...advancedStructuralGenerated.map((filename) =>
    `--js=${join(advancedStructuralBuildDirectory, filename)}`),
  `--externs=${join(root, 'closure-advanced.externs.js')}`,
  '--js_output_file=dist/closure-advanced-structural.js',
  '--compilation_level=ADVANCED',
  '--language_in=ECMASCRIPT_NEXT',
  '--language_out=ECMASCRIPT_2022',
  '--module_resolution=NODE',
  '--dependency_mode=PRUNE',
  `--entry_point=${join(advancedStructuralBuildDirectory, 'main')}`,
  '--warning_level=QUIET',
  '--charset=UTF-8',
]);

const advancedStructuralSource = readFileSync(
  join(distDirectory, 'closure-advanced-structural.js'),
  'utf8',
);
for (const selector of [
  'app-root',
  'fixture-card',
  'fixture-lazy-card',
  'structural-view-card',
  'structural-pure-bindings',
  'structural-let-bindings',
  'structural-namespaces',
  'structural-class-apis',
  'structural-query-apis',
]) {
  if (!advancedStructuralSource.includes(selector)) {
    throw new Error(`Structural ADVANCED output is missing ${selector}`);
  }
}
for (const contractName of [
  '__wakaruAngularDefinitions',
  '__wakaruAngularRoots',
  '__wakaruIvyRuntime',
  '__wakaruStructuralRuntime',
  'ɵɵdefineComponent',
]) {
  if (!advancedStructuralSource.includes(contractName)) {
    throw new Error(`Structural ADVANCED output is missing ${contractName}`);
  }
}
for (const excludedRole of [
  'ɵɵelementStart',
  'ɵɵtext',
  'ɵɵproperty',
  'ɵɵconditional',
  'ɵɵdeclareLet',
  'ɵɵdefer',
  'ɵɵdeferOnIdle',
  'ɵɵgetCurrentView',
  'ɵɵinterpolate',
  'ɵɵinterpolate1',
  'ɵɵnamespaceHTML',
  'ɵɵnamespaceMathML',
  'ɵɵnamespaceSVG',
  'ɵɵnextContext',
  'ɵɵpureFunction0',
  'ɵɵpureFunction1',
  'ɵɵreadContextLet',
  'ɵɵrepeater',
  'ɵɵrepeaterCreate',
  'ɵɵresetView',
  'ɵɵrestoreView',
  'ɵɵstoreLet',
]) {
  if (advancedStructuralSource.includes(excludedRole)) {
    throw new Error(
      `Structural ADVANCED output unexpectedly retained ${excludedRole}`,
    );
  }
}
