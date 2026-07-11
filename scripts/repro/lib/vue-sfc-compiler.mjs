export const VUE_SFC_COMPILE_PROFILES = Object.freeze([
  Object.freeze({
    name: "prod-inline",
    isProd: true,
    inlineTemplate: true,
    tier: "production-default",
  }),
  Object.freeze({
    name: "prod-external",
    isProd: true,
    inlineTemplate: false,
    tier: "production-fallback",
  }),
  Object.freeze({
    name: "dev-external",
    isProd: false,
    inlineTemplate: false,
    tier: "development",
  }),
]);

export function vueSfcCompileProfile(name) {
  const profile = VUE_SFC_COMPILE_PROFILES.find((candidate) => candidate.name === name);
  if (!profile) {
    const names = VUE_SFC_COMPILE_PROFILES.map((candidate) => candidate.name).join(", ");
    throw new Error(`unknown Vue SFC compile profile ${name}; expected one of: ${names}`);
  }
  return profile;
}

export function compileVueSfc({
  source,
  filename,
  compiler,
  profile,
  id,
  componentName = "__sfc__",
  includeFilename = false,
}) {
  const parsed = compiler.parse(source, { filename });
  if (parsed.errors.length) {
    throw new Error(`${filename}: ${formatCompilerErrors(parsed.errors)}`);
  }

  const descriptor = parsed.descriptor;
  if (descriptor.template?.src || descriptor.template?.lang) {
    throw new Error(
      `${filename}: Vue compiler profiles require resolved plain template content`,
    );
  }
  const compilerOptions = {
    cacheHandlers: true,
    hoistStatic: true,
  };
  const templateInlined = Boolean(
    profile.inlineTemplate && descriptor.scriptSetup && descriptor.template,
  );
  const compiledScript = descriptor.script || descriptor.scriptSetup
    ? compiler.compileScript(descriptor, {
        id,
        genDefaultAs: componentName,
        isProd: profile.isProd,
        inlineTemplate: profile.inlineTemplate,
        ...(templateInlined
          ? {
              templateOptions: {
                isProd: profile.isProd,
                compilerOptions,
              },
            }
          : {}),
      })
    : { content: `const ${componentName} = {}`, bindings: {} };

  const output = [compiledScript.content];
  if (descriptor.template && !templateInlined) {
    const template = compiler.compileTemplate({
      source: descriptor.template.content,
      filename,
      id,
      isProd: profile.isProd,
      compilerOptions: {
        ...compilerOptions,
        bindingMetadata: compiledScript.bindings,
      },
    });
    if (template.errors.length) {
      throw new Error(`${filename}: ${formatCompilerErrors(template.errors)}`);
    }
    output.push(template.code, `${componentName}.render = render;`);
  }

  if (includeFilename) {
    output.push(`${componentName}.__file = ${JSON.stringify(filename)};`);
  }
  output.push(`export default ${componentName};`);
  return output.join("\n\n");
}

function formatCompilerErrors(errors) {
  return errors.map((error) => error?.message ?? String(error)).join("; ");
}
