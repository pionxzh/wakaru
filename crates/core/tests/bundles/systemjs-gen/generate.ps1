# Generates SystemJS test fixtures from the source files in src/.
# Requires Node.js + npm (uses npx to fetch Rollup, SWC, and TypeScript on-the-fly).
#
# Usage:
#   cd crates/core/tests/bundles/systemjs-gen
#   powershell -ExecutionPolicy Bypass -File generate.ps1
#
# Generated outputs are checked into the repo so tests do not require Node.js.

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$rollupVersion = "4.29.1"
$babelCliVersion = "7.25.9"
$babelCoreVersion = "7.26.0"
$babelSystemjsVersion = "7.25.9"
$swcCliVersion = "0.7.9"
$swcCoreVersion = "1.15.3"
$typescriptVersion = "5.9.3"
$webpackVersion = "5.103.0"
$webpackCliVersion = "5.1.4"

if (Test-Path dist) {
    Remove-Item -Recurse -Force dist
}

Write-Host "=== Rollup $rollupVersion ==="
Write-Host "  preserve: System.register preserveModules output"
npx --yes "rollup@$rollupVersion" src/entry.js --format system --preserveModules --dir dist/preserve

Write-Host ""
Write-Host "=== Babel $babelCoreVersion ==="
Write-Host "  babel: @babel/plugin-transform-modules-systemjs compiler output"
npm install --no-save --no-package-lock --ignore-scripts "@babel/cli@$babelCliVersion" "@babel/core@$babelCoreVersion" "@babel/plugin-transform-modules-systemjs@$babelSystemjsVersion"
npx babel src --out-dir dist/babel --plugins "@babel/plugin-transform-modules-systemjs"

Write-Host ""
Write-Host "=== SWC $swcCoreVersion ==="
Write-Host "  swc: module.type=systemjs compiler output"
npx --yes -p "@swc/cli@$swcCliVersion" -p "@swc/core@$swcCoreVersion" swc src -d dist/swc --config-file swc.swcrc

Write-Host ""
Write-Host "=== TypeScript $typescriptVersion ==="
Write-Host "  tsc: --module system compiler output"
npx --yes -p "typescript@$typescriptVersion" tsc src-ts/entry.ts src-ts/dep.ts --module system --target es2018 --outDir dist/tsc

Write-Host ""
Write-Host "=== Webpack $webpackVersion ==="
Write-Host "  webpack: output.library.type=system wrapper"
npx --yes -p "webpack@$webpackVersion" -p "webpack-cli@$webpackCliVersion" webpack --config webpack.system.config.cjs

Write-Host ""
Write-Host "Done. Outputs in dist/*/"
