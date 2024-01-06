# Changelog

## [0.1.1](https://github.com/pionxzh/wakaru/compare/unpacker-v0.1.0...unpacker-v0.1.1) (2024-01-06)


### Bug Fixes

* handle parsing error in unpacker ([1a86293](https://github.com/pionxzh/wakaru/commit/1a8629343e5fc8284c82af591a64ce8164dd222c))

## [0.1.0](https://github.com/pionxzh/wakaru/compare/unpacker-v0.0.4...unpacker-v0.1.0) (2023-12-15)


### ⚠ BREAKING CHANGES

* bump Node.js to >= 18
* deprecate `@wakaru/unminify` and `@wakaru/unpacker` CLI in favor of `@wakaru/cli`

### Features

* bump Node.js to &gt;= 18 ([c36d0a0](https://github.com/pionxzh/wakaru/commit/c36d0a0176db56e98841051db264ab4c4f13739d))
* deprecate `@wakaru/unminify` and `@wakaru/unpacker` CLI in favor of `@wakaru/cli` ([be2012e](https://github.com/pionxzh/wakaru/commit/be2012e112145e0025cef7aa74a9686c0f952a6d))


### Bug Fixes

* improve error log ([df455eb](https://github.com/pionxzh/wakaru/commit/df455eb5fc4186d0d57d7ae5d676a8b45407ad64))

## [0.0.4](https://github.com/pionxzh/wakaru/compare/unpacker-v0.0.3...unpacker-v0.0.4) (2023-12-04)


### Features

* **babel-runtime:** add matcher for `taggedTemplateLiteral` and `taggedTemplateLiteralLoose` ([c0e2bee](https://github.com/pionxzh/wakaru/commit/c0e2beeb743f9188050b3a9ab18bf28fd70ddb4b))
* **playground:** let unpack run in worker ([05bf698](https://github.com/pionxzh/wakaru/commit/05bf698b5b1f5f4464422d07e78fcf8fe5956b29))


### Bug Fixes

* **cli:** paths out of cwd is not allowed ([7cee0c8](https://github.com/pionxzh/wakaru/commit/7cee0c8d461a12fb710a44722be043065cf072ed))
* **unpacker:** fix require.d to export should not introduce duplicate identifier ([50231b6](https://github.com/pionxzh/wakaru/commit/50231b626e61c1e078a52cd2fc8ed813bcbe6cd9))

## [0.0.3](https://github.com/pionxzh/wakaru/compare/unpacker-v0.0.2...unpacker-v0.0.3) (2023-11-19)


### Bug Fixes

* avoid removing output folder ([02f8d46](https://github.com/pionxzh/wakaru/commit/02f8d4631dcbe62f2a91b6b0b88811dd06d31039))
* move internal packages to devDeps ([da665a0](https://github.com/pionxzh/wakaru/commit/da665a09d4e2915fcc8d80e6f687c723160bf097))

## [0.0.2](https://github.com/pionxzh/wakaru/compare/unpacker-v0.0.1...unpacker-v0.0.2) (2023-11-19)


### Features

* add `un-iife` ([fba8056](https://github.com/pionxzh/wakaru/commit/fba805626f0f16b15c83cc333b77a46fbd3ee67c))
* babel runtime scan ([a2587de](https://github.com/pionxzh/wakaru/commit/a2587ded911cc61ca31c893851e7c4417015658b))
* **babel-helpers:** support `extends` and improve `objectDestructuringEmpty` ([c1957b6](https://github.com/pionxzh/wakaru/commit/c1957b63819f416237246bf6d53f19072ce93536))
* **babel-runtime:** support `interopRequireDefault` ([55b64fa](https://github.com/pionxzh/wakaru/commit/55b64fa2dcb57183476acb64b50cbcce40894a55))
* **babel-runtime:** support `interopRequireWildcard` ([bc0006c](https://github.com/pionxzh/wakaru/commit/bc0006ccf378859ed5b6fbc174947c3eb4ebffa2))
* build package ([2c281bc](https://github.com/pionxzh/wakaru/commit/2c281bc29af5e609bbd352b1bde4b14d5b3122e5))
* extract `@unminify/ast-utils` ([9c5fe11](https://github.com/pionxzh/wakaru/commit/9c5fe1147001edf1620fb3e1660010ee81ba2408))
* handle IIFE entry ([c06c21b](https://github.com/pionxzh/wakaru/commit/c06c21b495169d8321a115d7e235951285514a11))
* **iife:** detect various iife ([0356085](https://github.com/pionxzh/wakaru/commit/035608500def05031f24dc2c46e224eda986cac4))
* implement `wrapDeclarationWithExport` ([a2f4249](https://github.com/pionxzh/wakaru/commit/a2f42496c1b93726feaae15156e869a74b2df9f1))
* playground ([74075f4](https://github.com/pionxzh/wakaru/commit/74075f43e8c47aabe23fe6bd680fc57a31ede219))
* **playground:** support share link ([b1de7b2](https://github.com/pionxzh/wakaru/commit/b1de7b2cf3aab95ec619489a587f4bbd76c6e514))
* pre scan module meta ([70fde64](https://github.com/pionxzh/wakaru/commit/70fde646079d483dc4b91fff2d206c3ef786adc8))
* support minified enum ([f7b882e](https://github.com/pionxzh/wakaru/commit/f7b882e0f6e1a5b8223a85d5f159ba5c419a3fcf))
* **un-indirect-call:** implement indrect call replacement ([2594302](https://github.com/pionxzh/wakaru/commit/25943028817dfcf99c424afd867fbd4ffc246d84))
* unpack browserify ([6df2d3f](https://github.com/pionxzh/wakaru/commit/6df2d3f537e7799168f2bfcaea8534fb81e563e9))
* **unpacker:** implement cli ([e3d2777](https://github.com/pionxzh/wakaru/commit/e3d277770d20c11d5af09073d202fd6f4416f4c9))
* **unpacker:** implement webpack jsonp parsing ([309479e](https://github.com/pionxzh/wakaru/commit/309479ec082bd06daa4587e3648e3b3b82dfa3c6))
* webpack4 entry id detection ([7acb7f7](https://github.com/pionxzh/wakaru/commit/7acb7f7df7bc2fa581cda852104b0b8c92fd43f9))


### Bug Fixes

* **babel-runtime:** support `createForOfIteratorHelper` ([f2c08a3](https://github.com/pionxzh/wakaru/commit/f2c08a3e1730cf27aabb919d33c69229eb150db6))
* directly generate export statement ([a4d1951](https://github.com/pionxzh/wakaru/commit/a4d1951b33b9ea50ffae8b1b5fad96e8fa2fb323))
* fix reference check and identifier renaming ([244cb3e](https://github.com/pionxzh/wakaru/commit/244cb3ebfc89634b8b91c1efda7ecd4a687c6f94))
* fix type from `ast-types` ([50527d7](https://github.com/pionxzh/wakaru/commit/50527d708d1f17de1bae4d67fb650bbf84f86dd9))
* make jsonp support window["jsonp"] form ([2017731](https://github.com/pionxzh/wakaru/commit/20177314a155bcb04efa794005dba840f7cb2418))
* module mapping type and improve preview format ([d33ca82](https://github.com/pionxzh/wakaru/commit/d33ca82af900cf54f2626beddcb668713b8821ff))
* **playground:** improve the UI ([2b2615c](https://github.com/pionxzh/wakaru/commit/2b2615c7ebaf4aee98802adf01b27f9235a69851))
* rename parameter with respect to scope and property ([448595d](https://github.com/pionxzh/wakaru/commit/448595ddb83815b29136cb21e7ac32d13c470975))
* rename should respect scope ([571de1a](https://github.com/pionxzh/wakaru/commit/571de1a259cd292813dadb799aa2d29a599fa3ba))
* replace `require.d` without checking `exports` ([ebee03b](https://github.com/pionxzh/wakaru/commit/ebee03b921b9fb025dcf559b33000651ba07471a))
* **unpacker:** fix require.d replacment and add tests for it ([3fd6772](https://github.com/pionxzh/wakaru/commit/3fd677265202f3725776a094faeac994f7ae6463))
