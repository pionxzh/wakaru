// A pinned version still pulls every dependency through `^` ranges, so npm
// installs the newest compatible release of each: a pinned `@babel/core` or
// `@babel/preset-env` then lowers with the latest plugins and helper bodies,
// a combination no project that installed the pinned release ever ran.
// Installers pass `npm install --before <cutoff>` instead, so the tree
// resolves as of a cutoff after the newest spec's publish time. The window
// admits same-day patch releases, such as `@babel/runtime@7.12.18`, published
// an hour after 7.12.17 to fix exports that break under Node 17+.
export const RESOLUTION_WINDOW_DAYS = 1;

export function parseExactSpec(spec) {
  const match = /^((?:@[^/@]+\/)?[^/@]+)@(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$/.exec(spec);
  return match ? { name: match[1], version: match[2] } : null;
}

export function resolutionCutoff(publishTimes) {
  const newest = Math.max(...publishTimes.map((time) => Date.parse(time)));
  return new Date(newest + RESOLUTION_WINDOW_DAYS * 24 * 60 * 60 * 1000).toISOString();
}

// `viewTimes(name)` returns the stdout of `npm view <name> time --json`; each
// installer runs npm through its own platform-aware command runner.
export function releaseDateCutoff(specs, viewTimes) {
  const times = specs.map((spec) => {
    const parsed = parseExactSpec(spec);
    if (!parsed) throw new Error(`${spec} is not an exact version`);
    const time = JSON.parse(viewTimes(parsed.name))[parsed.version];
    if (!time) throw new Error(`npm has no publish time for ${spec}`);
    return time;
  });
  return resolutionCutoff(times);
}
