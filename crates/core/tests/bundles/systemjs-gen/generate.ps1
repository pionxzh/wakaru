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
$swcCliVersion = "0.7.9"
$swcCoreVersion = "1.15.3"
$typescriptVersion = "5.9.3"

if (Test-Path dist) {
    Remove-Item -Recurse -Force dist
}

Write-Host "=== Rollup $rollupVersion ==="
Write-Host "  preserve: System.register preserveModules output"
npx --yes "rollup@$rollupVersion" src/entry.js --format system --preserveModules --dir dist/preserve

Write-Host ""
Write-Host "=== SWC $swcCoreVersion ==="
Write-Host "  swc: module.type=systemjs compiler output"
npx --yes -p "@swc/cli@$swcCliVersion" -p "@swc/core@$swcCoreVersion" swc src -d dist/swc --config-file swc.swcrc

Write-Host ""
Write-Host "=== TypeScript $typescriptVersion ==="
Write-Host "  tsc: --module system compiler output"
npx --yes -p "typescript@$typescriptVersion" tsc src-ts/entry.ts src-ts/dep.ts --module system --target es2018 --outDir dist/tsc

Write-Host ""
Write-Host "Done. Outputs in dist/*/"
