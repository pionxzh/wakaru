#!/bin/bash

pnpm exec tsup --silent --onSuccess "node $(dirname "$0")/../dist/cli.cjs $*"
