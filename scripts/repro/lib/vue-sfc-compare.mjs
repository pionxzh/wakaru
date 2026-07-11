// Build a small JavaScript program that links a <script setup> event handler
// to the template-local bindings used at its call site. The regular repro
// normalizer can then alpha-rename the program without having to parse an SFC.

function collectPatternBindings(pattern, bindings) {
  if (!pattern) return;

  switch (pattern.type) {
    case "Identifier":
      bindings.push(pattern.name);
      break;
    case "ObjectPattern":
      for (const property of pattern.properties) {
        if (property.type === "RestElement") {
          collectPatternBindings(property.argument, bindings);
        } else {
          collectPatternBindings(property.value, bindings);
        }
      }
      break;
    case "ArrayPattern":
      for (const element of pattern.elements) collectPatternBindings(element, bindings);
      break;
    case "AssignmentPattern":
      collectPatternBindings(pattern.left, bindings);
      break;
    case "RestElement":
      collectPatternBindings(pattern.argument, bindings);
      break;
    case "TSParameterProperty":
      collectPatternBindings(pattern.parameter, bindings);
      break;
  }
}

function parsePatternBindings(content, babelParser, plugins) {
  try {
    const expression = babelParser.parseExpression(`(${content}) => 0`, { plugins });
    const bindings = [];
    for (const parameter of expression.params) collectPatternBindings(parameter, bindings);
    return bindings;
  } catch {
    return [];
  }
}

function extendScope(scope, bindings) {
  const shadowed = new Set(bindings);
  return [...scope.filter((name) => !shadowed.has(name)), ...bindings];
}

function eventExpression(node, eventName) {
  if (node.type !== 1) return null;
  for (const property of node.props ?? []) {
    if (
      property.type === 7
      && property.name === "on"
      && property.arg?.isStatic
      && property.arg.content === eventName
      && property.exp?.content
    ) {
      return property.exp.content;
    }
  }
  return null;
}

function slotBindings(node, babelParser, plugins) {
  if (node.type !== 1) return [];
  const bindings = [];
  for (const property of node.props ?? []) {
    if (property.type === 7 && property.name === "slot" && property.exp?.content) {
      bindings.push(...parsePatternBindings(property.exp.content, babelParser, plugins));
    }
  }
  return bindings;
}

function findScopedEvent(node, scope, eventName, babelParser, plugins) {
  const localScope = extendScope(scope, slotBindings(node, babelParser, plugins));
  const expression = eventExpression(node, eventName);
  if (expression && localScope.length > 0) return { expression, scopeBindings: localScope };

  for (const child of node.children ?? []) {
    const match = findScopedEvent(child, localScope, eventName, babelParser, plugins);
    if (match) return match;
  }
  return null;
}

function parserPlugins(lang) {
  const plugins = [];
  if (lang === "ts" || lang === "tsx") plugins.push("typescript");
  if (lang === "jsx" || lang === "tsx") plugins.push("jsx");
  return plugins;
}

function handlerDeclaration(content, ast, handlerName) {
  for (const statement of ast.program.body) {
    if (statement.type === "FunctionDeclaration" && statement.id?.name === handlerName) {
      return content.slice(statement.start, statement.end);
    }
    if (statement.type !== "VariableDeclaration") continue;
    for (const declaration of statement.declarations) {
      if (
        declaration.id.type === "Identifier"
        && declaration.id.name === handlerName
        && (declaration.init?.type === "ArrowFunctionExpression"
          || declaration.init?.type === "FunctionExpression")
      ) {
        return `const ${handlerName} = ${content.slice(declaration.init.start, declaration.init.end)};`;
      }
    }
  }
  return null;
}

/**
 * Extract one scoped template event and its top-level <script setup> handler as
 * a parseable JavaScript program. Returns null for forms that cannot be linked
 * safely; callers should then use their normal strict comparison.
 */
export function linkedEventHandlerProgram(source, options) {
  const {
    compiler,
    babelParser,
    eventName = "click",
    filename = "Compare.vue",
  } = options;
  let parsed;
  try {
    parsed = compiler.parse(source, { filename });
  } catch {
    return null;
  }
  if (parsed.errors?.length > 0) return null;

  const { scriptSetup, template } = parsed.descriptor;
  if (!scriptSetup || !template?.ast) return null;
  const plugins = parserPlugins(scriptSetup.lang);
  const event = findScopedEvent(template.ast, [], eventName, babelParser, plugins);
  if (!event) return null;

  let expression;
  try {
    expression = babelParser.parseExpression(event.expression, { plugins });
  } catch {
    return null;
  }
  if (expression.type !== "CallExpression" || expression.callee.type !== "Identifier") return null;

  let scriptAst;
  try {
    scriptAst = babelParser.parse(scriptSetup.content, {
      sourceType: "module",
      plugins,
    });
  } catch {
    return null;
  }
  const declaration = handlerDeclaration(scriptSetup.content, scriptAst, expression.callee.name);
  if (!declaration) return null;

  return {
    handlerName: expression.callee.name,
    expression: event.expression,
    scopeBindings: event.scopeBindings,
    program: [
      declaration,
      `const __wakaru_event = (${event.scopeBindings.join(", ")}) => (${event.expression});`,
    ].join("\n"),
  };
}
