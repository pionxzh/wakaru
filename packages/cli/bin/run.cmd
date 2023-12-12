@echo off

pnpm exec tsup --silent --onSuccess "node %~dp0\..\dist\cli.cjs %*"
