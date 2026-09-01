#!/usr/bin/env python3
"""Generate a synthetic app fixture for the synthetic-bundle harness.

Deterministic given a seed: samples a stratified package set from pool.json,
emits a layered-DAG app under workspace/apps/syn<seed>/src/, and writes a
manifest.json recording the sampled packages.

App code is plain ESM JS (no JSX/SFC) so every bundler in the matrix can
build it without transform plugins. Vendor packages are imported as
namespaces and kept live via Object.keys(ns) sinks.
"""
import json
import random
import sys
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
POOL = json.loads((HARNESS / "pool.json").read_text())["categories"]

NOUNS = ["order", "invoice", "profile", "ticket", "session", "catalog",
         "shipment", "account", "report", "wallet", "booking", "campaign"]
VERBS = ["submit", "cancel", "refresh", "archive", "validate", "merge",
         "export", "approve", "sync", "publish"]
AREAS = ["checkout", "dashboard", "settings", "search", "billing",
         "inbox", "admin", "onboarding", "analytics", "support"]
KINDS = ["page", "util", "service", "store", "widget"]


def _fn_body(rng, mod_prefix):
    noun = rng.choice(NOUNS)
    verb = rng.choice(VERBS)
    area = rng.choice(AREAS)
    style = rng.randrange(4)
    name = f"{verb}{noun.capitalize()}{rng.randrange(100)}"
    if style == 0:
        body = (
            f'export function {name}(input) {{\n'
            f'  if (input == null) throw new Error("{area}: {noun} payload is required");\n'
            f'  const route = "/api/v{rng.randrange(1, 4)}/{area}/{noun}s/{verb}";\n'
            f'  return route + ":" + JSON.stringify(input).length;\n'
            f'}}\n'
        )
    elif style == 1:
        body = (
            f'export function {name}(items) {{\n'
            f'  const labels = ["{verb.capitalize()} {noun}", "{rng.choice(VERBS).capitalize()} all", "Retry"];\n'
            f'  return (items || []).map((it, i) => labels[i % labels.length] + "#" + it);\n'
            f'}}\n'
        )
    elif style == 2:
        body = (
            f'export const {name} = (state) => ({{\n'
            f'  ...state,\n'
            f'  {noun}Count: (state.{noun}Count || 0) + {rng.randrange(1, 5)},\n'
            f'  lastAction: "{area}/{verb}",\n'
            f'}});\n'
        )
    else:
        body = (
            f'export async function {name}(client) {{\n'
            f'  const res = await client.fetch("/{area}/{noun}/" + Date.now());\n'
            f'  if (!res) throw new Error("failed to {verb} {noun} in {mod_prefix}");\n'
            f'  return res;\n'
            f'}}\n'
        )
    return name, body


def sample_packages(rng):
    picks = []
    for cat, pkgs in POOL.items():
        n = 1 if rng.random() < 0.8 else 2
        picks.extend(rng.sample(pkgs, min(n, len(pkgs))))
    # trim/grow to 8-12
    target = rng.randint(8, 12)
    rng.shuffle(picks)
    return sorted(set(picks[:target]))


def generate(seed, apps_dir):
    rng = random.Random(seed)
    name = f"syn{seed}"
    app_dir = Path(apps_dir) / name
    src = app_dir / "src"
    src.mkdir(parents=True, exist_ok=True)

    packages = sample_packages(rng)
    n_modules = rng.randint(12, 36)
    mod_names = []
    for i in range(n_modules):
        kind = rng.choice(KINDS)
        mod_names.append(f"{kind}_{rng.choice(AREAS)}_{i}")

    # assign each vendor package to 1-3 modules
    vendor_of = {m: [] for m in mod_names}
    for pkg in packages:
        for m in rng.sample(mod_names, rng.randint(1, min(3, n_modules))):
            vendor_of[m].append(pkg)

    # ~25% of app modules are CommonJS (realistic mixed codebase; defeats
    # full scope-hoisting so app modules survive as units in prod bundles).
    # CJS modules take no app deps to avoid require(ESM) interop edge cases.
    cjs_set = set(rng.sample(range(n_modules), max(1, n_modules // 4)))

    imported = set()
    for i, mod in enumerate(mod_names):
        is_cjs = i in cjs_set
        lines = []
        dep_exports = []
        if i > 0 and not is_cjs:
            n_deps = rng.randint(1, min(3, i))
            for j in sorted(rng.sample(range(i), n_deps)):
                dep = mod_names[j]
                imported.add(dep)
                lines.append(f'import * as {dep} from "./{dep}.js";')
                dep_exports.append(dep)
        vnd_names = []
        for k, pkg in enumerate(vendor_of[mod]):
            v = f"vnd{k}"
            vnd_names.append(v)
            if is_cjs:
                lines.append(f'const {v} = require("{pkg}");')
            else:
                lines.append(f'import * as {v} from "{pkg}";')
        lines.append("")
        fn_names = []
        for _ in range(rng.randint(2, 4)):
            fn, body = _fn_body(rng, mod)
            if is_cjs:
                body = body.replace("export function ", "function ", 1)
                body = body.replace("export const ", "const ", 1)
                body = body.replace("export async function ", "async function ", 1)
            fn_names.append(fn)
            lines.append(body)
        sinks = [f"Object.keys({v}).length" for v in vnd_names]
        sinks += [f"Object.keys({d}).length" for d in dep_exports]
        sinks += [f"{fn}.name.length" for fn in fn_names]
        weight_expr = " + ".join(sinks or ["0"])
        if is_cjs:
            exports = ", ".join(fn_names + [f"weight_{i}"])
            lines.append(f"const weight_{i} = {weight_expr};")
            lines.append(f"module.exports = {{ {exports} }};")
        else:
            lines.append(f"export const weight_{i} = {weight_expr};")
        (src / f"{mod}.js").write_text("\n".join(lines) + "\n")

    roots = [m for m in mod_names if m not in imported]
    # ~1/3 of roots load via dynamic import() (route-style code splitting),
    # so bundlers emit real chunks and app modules keep boundaries.
    n_dyn = len(roots) // 3 if len(roots) >= 3 else 0
    dyn_roots = rng.sample(roots, n_dyn) if n_dyn else []
    static_roots = [m for m in roots if m not in dyn_roots]

    entry = [f'import * as {m} from "./{m}.js";' for m in static_roots]
    entry.append("")
    if static_roots:
        entry.append("const total = " + " + ".join(
            f"{m}.weight_{mod_names.index(m)}" for m in static_roots) + ";")
    else:
        entry.append("const total = 0;")
    if dyn_roots:
        entry.append("const lazy = Promise.all([")
        for m in dyn_roots:
            entry.append(f'  import("./{m}.js"),')
        entry.append("]).then((mods) => mods.reduce((acc, m) => acc + Object.keys(m).length, 0));")
        entry.append('lazy.then((n) => console.log("lazy routes ready", n));')
    entry.append('globalThis.__SYN_APP__ = { total };')
    entry.append('console.log("syn app ready", total);')
    (src / "main.js").write_text("\n".join(entry) + "\n")

    manifest = {
        "seed": seed,
        "name": name,
        "packages": packages,
        "n_modules": n_modules + 1,
        "roots": len(roots),
        "dynamic_roots": len(dyn_roots),
        "cjs_modules": len(cjs_set),
    }
    (app_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


if __name__ == "__main__":
    seed = int(sys.argv[1])
    apps = sys.argv[2] if len(sys.argv) > 2 else str(HARNESS / "workspace" / "apps")
    m = generate(seed, apps)
    print(json.dumps(m, indent=2))
