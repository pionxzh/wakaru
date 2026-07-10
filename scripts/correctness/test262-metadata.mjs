const knownFlags = new Set([
  "onlyStrict",
  "noStrict",
  "module",
  "raw",
  "async",
  "generated",
  "CanBlockIsFalse",
  "CanBlockIsTrue",
  "non-deterministic",
  "explicit-resource-management",
]);

const knownNegativePhases = new Set(["parse", "early", "resolution", "runtime"]);

export class Test262MetadataError extends Error {
  constructor(message) {
    super(message);
    this.name = "Test262MetadataError";
  }
}

export function parseTestMetadata(source) {
  const start = source.indexOf("/*---");
  if (start < 0) {
    throw new Test262MetadataError("missing Test262 metadata start marker");
  }
  const contentStart = start + "/*---".length;
  const relativeEnd = source.slice(contentStart).indexOf("---*/");
  if (relativeEnd < 0) {
    throw new Test262MetadataError("missing Test262 metadata end marker");
  }

  const raw = source
    .slice(contentStart, contentStart + relativeEnd)
    .replaceAll("\r\n", "\n")
    .replaceAll("\r", "\n");
  const fields = scanTopLevelFields(raw);
  const flags = parseListField(fields, "flags");
  for (const flag of flags) {
    if (!knownFlags.has(flag)) {
      throw new Test262MetadataError(`unknown Test262 flag \`${flag}\``);
    }
  }
  if (new Set(flags).size !== flags.length) {
    throw new Test262MetadataError("Test262 metadata contains duplicate flags");
  }
  validateFlagCombinations(flags);

  return {
    esid: parseOptionalScalar(fields, "esid"),
    features: parseListField(fields, "features"),
    includes: parseListField(fields, "includes"),
    flags,
    negative: parseNegative(fields),
    raw,
  };
}

export function runnableVariants(metadata) {
  const raw = metadata.flags.includes("raw");
  if (metadata.flags.includes("module")) {
    return [
      {
        name: raw ? "raw-module" : "module",
        strict: true,
        module: true,
        ...(raw ? { raw: true } : {}),
      },
    ];
  }
  if (raw) {
    return [{ name: "raw-script", strict: false, raw: true }];
  }
  if (metadata.flags.includes("onlyStrict")) {
    return [{ name: "strict", strict: true }];
  }
  if (metadata.flags.includes("noStrict")) {
    return [{ name: "sloppy", strict: false }];
  }
  return [
    { name: "sloppy", strict: false },
    { name: "strict", strict: true },
  ];
}

function validateFlagCombinations(flags) {
  const unique = new Set(flags);
  const onlyStrict = unique.has("onlyStrict");
  const noStrict = unique.has("noStrict");
  if (onlyStrict && noStrict) {
    throw new Test262MetadataError(
      "Test262 metadata cannot combine `onlyStrict` and `noStrict`",
    );
  }
  if (unique.has("raw") && (onlyStrict || noStrict)) {
    throw new Test262MetadataError("Test262 `raw` tests cannot request strictness injection");
  }
  if (unique.has("module") && (onlyStrict || noStrict)) {
    throw new Test262MetadataError("Test262 module tests are inherently strict");
  }
}

function scanTopLevelFields(raw) {
  const lines = raw.split("\n");
  const fields = new Map();
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim().length === 0 || line.trimStart().startsWith("#") || /^\s/.test(line)) {
      continue;
    }
    const match = line.match(/^([A-Za-z0-9_-]+):(?:[ \t]*(.*))?$/);
    if (!match) {
      throw new Test262MetadataError(`invalid Test262 metadata line: ${line}`);
    }
    const [, key, inline = ""] = match;
    if (fields.has(key)) {
      throw new Test262MetadataError(`duplicate Test262 metadata field \`${key}\``);
    }
    const block = [];
    let cursor = index + 1;
    while (cursor < lines.length) {
      const candidate = lines[cursor];
      if (candidate.length > 0 && !/^\s/.test(candidate)) {
        break;
      }
      block.push(candidate);
      cursor += 1;
    }
    fields.set(key, { inline: inline.trim(), block });
    index = cursor - 1;
  }
  return fields;
}

function parseListField(fields, key) {
  const field = fields.get(key);
  if (!field) {
    return [];
  }
  if (field.inline) {
    return parseFlowSequence(field.inline, key);
  }
  const values = [];
  for (const line of field.block) {
    if (line.trim().length === 0 || line.trimStart().startsWith("#")) {
      continue;
    }
    const match = line.match(/^\s+-\s+(.+?)\s*$/);
    if (!match) {
      throw new Test262MetadataError(`\`${key}\` must be a YAML sequence of strings`);
    }
    values.push(parseScalar(match[1], `${key} entry`));
  }
  return values;
}

function parseFlowSequence(source, key) {
  const match = source.match(/^\[([\s\S]*)\](?:\s+#.*)?$/);
  if (!match) {
    throw new Test262MetadataError(`\`${key}\` must be a YAML sequence of strings`);
  }
  const body = match[1].trim();
  if (!body) {
    return [];
  }
  return splitFlowItems(body).map((item) => parseScalar(item, `${key} entry`));
}

function parseNegative(fields) {
  const field = fields.get("negative");
  if (!field) {
    return null;
  }
  const values = new Map();
  if (field.inline) {
    const match = field.inline.match(/^\{([\s\S]*)\}(?:\s+#.*)?$/);
    if (!match) {
      throw new Test262MetadataError("`negative` must be a YAML mapping");
    }
    for (const entry of splitFlowItems(match[1])) {
      const pair = entry.match(/^([A-Za-z]+)\s*:\s*(.+)$/);
      if (!pair) {
        throw new Test262MetadataError("`negative` must contain string phase and type fields");
      }
      setMappingValue(values, pair[1], pair[2], "negative");
    }
  } else {
    for (const line of field.block) {
      if (line.trim().length === 0 || line.trimStart().startsWith("#")) {
        continue;
      }
      const pair = line.match(/^\s+([A-Za-z]+):\s*(.+?)\s*$/);
      if (!pair) {
        throw new Test262MetadataError("`negative` must contain string phase and type fields");
      }
      setMappingValue(values, pair[1], pair[2], "negative");
    }
  }

  for (const key of values.keys()) {
    if (!new Set(["phase", "type"]).has(key)) {
      throw new Test262MetadataError(`unknown Test262 negative field \`${key}\``);
    }
  }
  const phase = values.has("phase") ? parseScalar(values.get("phase"), "negative.phase") : null;
  const type = values.has("type") ? parseScalar(values.get("type"), "negative.type") : null;
  if (!phase || !type) {
    throw new Test262MetadataError("Test262 negative metadata requires phase and type");
  }
  if (!knownNegativePhases.has(phase)) {
    throw new Test262MetadataError(`unknown Test262 negative phase \`${phase}\``);
  }
  return { phase, type };
}

function parseOptionalScalar(fields, key) {
  const field = fields.get(key);
  if (!field) {
    return null;
  }
  if (!field.inline || field.block.some((line) => line.trim().length > 0)) {
    throw new Test262MetadataError(`\`${key}\` must be a string`);
  }
  return parseScalar(field.inline, key);
}

function setMappingValue(values, key, value, field) {
  if (values.has(key)) {
    throw new Test262MetadataError(`duplicate ${field} field \`${key}\``);
  }
  values.set(key, value);
}

function parseScalar(source, label) {
  let value = source.trim();
  if (value.startsWith("'")) {
    if (!value.endsWith("'")) {
      throw new Test262MetadataError(`${label} has an unterminated quoted string`);
    }
    value = value.slice(1, -1).replaceAll("''", "'");
  } else if (value.startsWith('"')) {
    try {
      value = JSON.parse(value);
    } catch {
      throw new Test262MetadataError(`${label} has an invalid quoted string`);
    }
  } else {
    value = value.replace(/\s+#.*$/, "").trim();
  }
  if (!value || typeof value !== "string" || /^(?:null|~)$/i.test(value)) {
    throw new Test262MetadataError(`${label} must be a non-empty string`);
  }
  return value;
}

function splitFlowItems(source) {
  const items = [];
  let quote = null;
  let escaped = false;
  let start = 0;
  for (let index = 0; index < source.length; index += 1) {
    const char = source[index];
    if (quote === '"' && escaped) {
      escaped = false;
      continue;
    }
    if (quote === '"' && char === "\\") {
      escaped = true;
      continue;
    }
    if (quote && char === quote) {
      if (quote === "'" && source[index + 1] === "'") {
        index += 1;
      } else {
        quote = null;
      }
      continue;
    }
    if (!quote && (char === "'" || char === '"')) {
      quote = char;
      continue;
    }
    if (!quote && char === ",") {
      items.push(source.slice(start, index).trim());
      start = index + 1;
    }
  }
  if (quote) {
    throw new Test262MetadataError("unterminated quoted string in flow sequence");
  }
  items.push(source.slice(start).trim());
  if (items.some((item) => item.length === 0)) {
    throw new Test262MetadataError("empty entry in flow sequence");
  }
  return items;
}
