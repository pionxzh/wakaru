#!/usr/bin/env python3
"""Synthetic-bundle harness orchestrator for unpacker verification.

Per fixture (seeded synthetic app) and matrix variant (bundler x mode):
  generate app -> build bundle (+source map) -> wakaru --unpack --provenance
  -> verify the bundle actually split into modules.

Usage:
  python3 run_harness.py --seeds 1,2,3 --bundlers esbuild,webpack5,rollup \
      --modes prod,dev [--wakaru /path/to/wakaru]

The harness exercises real bundler output shapes (esbuild --format=iife /
--splitting, webpack5 prod/dev, rollup iife / code-split esm) against the
unpacker. Detection evaluation on these fixtures lives in the detector
research repo, not here.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
WORKSPACE = HARNESS / "workspace"
APPS = WORKSPACE / "apps"
REPO_ROOT = HARNESS.parent.parent
DEFAULT_WAKARU = REPO_ROOT / "target" / "dev-release" / "wakaru"

sys.path.insert(0, str(HARNESS))
from gen_app import generate  # noqa: E402

POOL = json.loads((HARNESS / "pool.json").read_text())


def sh(cmd, cwd=None, env=None, label=""):
    e = dict(os.environ)
    if env:
        e.update(env)
    r = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True)
    if r.returncode != 0:
        print(f"    FAIL [{label}] {' '.join(map(str, cmd))}")
        tail = (r.stderr or r.stdout or "").strip().splitlines()[-8:]
        for line in tail:
            print(f"      {line}")
    return r


def ensure_workspace():
    WORKSPACE.mkdir(parents=True, exist_ok=True)
    pkg_json = WORKSPACE / "package.json"
    if not pkg_json.exists():
        deps = {p: "*" for cat in POOL["categories"].values() for p in cat}
        deps.update({p: "*" for p in POOL["tooling"]})
        pkg_json.write_text(json.dumps({
            "name": "wakaru-harness-workspace",
            "private": True,
            "dependencies": deps,
        }, indent=2))
    if not (WORKSPACE / "node_modules").exists():
        print("Installing package pool (one-time npm install)...")
        r = sh(["npm", "install", "--no-audit", "--no-fund", "--loglevel=error"],
               cwd=WORKSPACE, label="npm install")
        if r.returncode != 0:
            sys.exit("npm install failed; cannot continue")
        print("  installed.")


def build_variant(app_dir, bundler, mode, split):
    entry = app_dir / "src" / "main.js"
    dist = app_dir / "dist" / f"{bundler}-{mode}"
    if dist.exists():
        shutil.rmtree(dist)
    dist.mkdir(parents=True)
    out = dist / "bundle.js"
    npx = ["npx", "--prefix", str(WORKSPACE)]
    if bundler == "esbuild":
        nodeenv = "production" if mode == "prod" else "development"
        cmd = npx + ["esbuild", str(entry), "--bundle", "--sourcemap",
                     "--platform=browser", "--target=es2017",
                     "--log-level=error",
                     f'--define:process.env.NODE_ENV="{nodeenv}"']
        if split:
            cmd += ["--splitting", "--format=esm", f"--outdir={dist}"]
        else:
            cmd += ["--format=iife", f"--outfile={out}"]
        if mode == "prod":
            cmd.append("--minify")
        r = sh(cmd, cwd=WORKSPACE, label=f"esbuild {mode}")
    elif bundler == "webpack5":
        wmode = "production" if mode == "prod" else "development"
        cmd = npx + ["webpack", "--config", str(HARNESS / "harness.webpack.cjs"),
                     "--env", f"entry={entry}", "--env", f"outdir={dist}",
                     "--env", f"mode={wmode}"]
        r = sh(cmd, cwd=WORKSPACE, label=f"webpack5 {mode}")
    elif bundler == "rollup":
        # config must sit inside the workspace so its plugin imports resolve
        # against workspace/node_modules
        cfg = WORKSPACE / "rollup.config.harness.mjs"
        shutil.copy(HARNESS / "rollup.config.harness.mjs", cfg)
        cmd = npx + ["rollup", "-c", str(cfg)]
        env = {
            "H_ENTRY": str(entry),
            "H_MODE": "production" if mode == "prod" else "development",
            "H_MIN": "1" if mode == "prod" else "0",
        }
        if split:
            env["H_OUTDIR"] = str(dist)
        else:
            env["H_OUT"] = str(out)
        r = sh(cmd, cwd=WORKSPACE, label=f"rollup {mode}", env=env)
    else:
        raise ValueError(f"unknown bundler {bundler}")
    if r.returncode != 0 or not any(dist.glob("*.js")):
        return None
    return dist


def unpack(wakaru, dist, app_dir, bundler, mode):
    unpacked = app_dir / "unpacked" / f"{bundler}-{mode}"
    if unpacked.exists():
        shutil.rmtree(unpacked)
    # Pass the whole dist dir: directory inputs are expanded recursively and
    # non-bundle files are skipped, so single-file and code-split variants
    # take the same path and provenance covers every chunk.
    r = sh([str(wakaru), "--unpack", "--provenance", str(dist),
            "-o", str(unpacked), "--force"], label="wakaru unpack")
    if r.returncode != 0:
        return None
    return unpacked


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", default="1,2,3")
    ap.add_argument("--bundlers", default="esbuild,webpack5,rollup")
    ap.add_argument("--modes", default="prod,dev")
    ap.add_argument("--wakaru", default=str(DEFAULT_WAKARU))
    args = ap.parse_args()

    wakaru = Path(args.wakaru)
    if not wakaru.exists():
        sys.exit(f"wakaru binary not found at {wakaru}; build with "
                 f"`cargo build --profile dev-release -p wakaru-cli`")

    ensure_workspace()
    seeds = [int(s) for s in args.seeds.split(",")]
    bundlers = args.bundlers.split(",")
    modes = args.modes.split(",")

    results = []
    for seed in seeds:
        manifest = generate(seed, APPS)
        app_dir = APPS / manifest["name"]
        split = manifest.get("dynamic_roots", 0) > 0
        print(f"\n=== {manifest['name']}: {manifest['n_modules']} app modules "
              f"({manifest.get('cjs_modules', 0)} cjs, {manifest.get('dynamic_roots', 0)} dyn roots), "
              f"{len(manifest['packages'])} packages: {', '.join(manifest['packages'])}")
        for bundler in bundlers:
            for mode in modes:
                name = f"{manifest['name']}-{bundler}-{mode}"
                print(f"  [{name}]")
                dist = build_variant(app_dir, bundler, mode, split)
                if dist is None:
                    results.append((name, "build-failed"))
                    continue
                unpacked = unpack(wakaru, dist, app_dir, bundler, mode)
                if unpacked is None:
                    results.append((name, "unpack-failed"))
                    continue
                n_flat = len(list(unpacked.glob("*.js")))
                n_all = len(list(unpacked.rglob("*.js")))
                print(f"    unpacked {n_all} modules ({n_flat} top-level)")
                if n_all > n_flat * 2:
                    # nested output = original paths leaked into the bundle
                    # (dev builds), wakaru reproduced the directory tree
                    results.append((name, f"ok-nested({n_all})"))
                elif n_all > 1:
                    results.append((name, f"ok({n_all})"))
                else:
                    # a single output file means the bundle was not split
                    results.append((name, "not-split"))

    print("\n=== Harness summary ===")
    failed = False
    for name, status in results:
        print(f"  {name:32s} {status}")
        if status in ("build-failed", "unpack-failed", "not-split"):
            failed = True
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
