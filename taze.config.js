import { defineConfig } from 'taze'

export default defineConfig({
    mode: 'minor',
    recursive: true,
    // ignore packages from bumping
    exclude: [
    ],
    ignorePaths: [
        'testcases/**',
    ],
    // fetch latest package info from registry without cache
    force: false,
    // write to package.json
    // write: true,
    interactive: true,
    // run `pnpm install` right after bumping
    install: true,
    // override with different bumping mode for each package
    packageMode: {
        globby: 'minor',
        prettier: 'minor',
    },
})
