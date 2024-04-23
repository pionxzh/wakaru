# Changelog

## [0.0.10](https://github.com/pionxzh/wakaru/compare/cli-v0.0.9...cli-v0.0.10) (2024-04-23)


### Features

* added support for `--perf-output` in CLI ([e4ce13c](https://github.com/pionxzh/wakaru/commit/e4ce13c8c8128fe471f22d06cd4fec2880ceab9a))

## [0.0.9](https://github.com/pionxzh/wakaru/compare/cli-v0.0.8...cli-v0.0.9) (2024-02-16)


### Bug Fixes

* **cli:** worker pool size should not more than file count ([63c6f02](https://github.com/pionxzh/wakaru/commit/63c6f02c16d397bf73d66d937e7a8cbc5e280f34))
* reduce cli size by enabling minify ([25cd173](https://github.com/pionxzh/wakaru/commit/25cd173c07d1f0731d0446e4d724dadb1f072684))

## [0.0.8](https://github.com/pionxzh/wakaru/compare/cli-v0.0.7...cli-v0.0.8) (2024-01-21)


### Bug Fixes

* **cli:** remove debug log ([627870c](https://github.com/pionxzh/wakaru/commit/627870cbc7fff7ecbb030566d6c7377e2b5c5018))

## [0.0.7](https://github.com/pionxzh/wakaru/compare/cli-v0.0.6...cli-v0.0.7) (2024-01-21)


### Features

* **cli:** add file size checker and warning ([f76be29](https://github.com/pionxzh/wakaru/commit/f76be2928146031d00a35529052e516c3f5ea47a))
* support async transformation ([9e37839](https://github.com/pionxzh/wakaru/commit/9e37839a731f492cf7719f4a66e8feced975fc66))

## [0.0.6](https://github.com/pionxzh/wakaru/compare/cli-v0.0.5...cli-v0.0.6) (2024-01-09)


### Bug Fixes

* **cli:** `perf.json` will be generated at the common base dir when there are multiple output directories ([7997c83](https://github.com/pionxzh/wakaru/commit/7997c836ea6ec46ba6f51630b303c3cf00e51911)), closes [#85](https://github.com/pionxzh/wakaru/issues/85)

## [0.0.5](https://github.com/pionxzh/wakaru/compare/cli-v0.0.4...cli-v0.0.5) (2024-01-06)


### Bug Fixes

* **cli:** improve unpacker log for multiple files ([96e0ef3](https://github.com/pionxzh/wakaru/commit/96e0ef3663e28fa46c32da34ec7056fa9b8ea51f))

## [0.0.4](https://github.com/pionxzh/wakaru/compare/cli-v0.0.3...cli-v0.0.4) (2024-01-01)


### Features

* **cli:** add perf stats for unpacker ([395cb7f](https://github.com/pionxzh/wakaru/commit/395cb7f9d0b47ecf67a9c23d715028eedbd16348)), closes [#84](https://github.com/pionxzh/wakaru/issues/84)


### Bug Fixes

* **cli:** fix missing validation message ([52cb975](https://github.com/pionxzh/wakaru/commit/52cb975784b03438ed5831f2bf5a52ea67939aac))
* **cli:** perf should be generated no matter what feature was chosen ([14e8928](https://github.com/pionxzh/wakaru/commit/14e8928795b0cbd73f79352e244d34f5c6a9336a))

## [0.0.3](https://github.com/pionxzh/wakaru/compare/cli-v0.0.2...cli-v0.0.3) (2023-12-30)


### Features

* **cli:** support path auto-completion ([d5b731c](https://github.com/pionxzh/wakaru/commit/d5b731cb9fba2941ac4e74035c5d3e00dbfe2c4e))


### Bug Fixes

* **cli:** fix output path validation throw "no such file or directory" ([f4475d8](https://github.com/pionxzh/wakaru/commit/f4475d865aab57d13b0d6d9683fe60c229393849))
* **cli:** handle `perf.json` path when output is overriden ([332c84b](https://github.com/pionxzh/wakaru/commit/332c84ba641cac2c2a08540a6098accb9be28f92)), closes [#85](https://github.com/pionxzh/wakaru/issues/85)
* **cli:** handle path completion at base ([1a5fa1e](https://github.com/pionxzh/wakaru/commit/1a5fa1efa727660cbad57dcf09f7dbe8654447d0))
* **cli:** input should all use `resolveFileGlob` ([3ef39a6](https://github.com/pionxzh/wakaru/commit/3ef39a6aa8dfe1d5e2c03b0965824528ac17c869))
* **cli:** interative mode's path validation should accept glob pattern ([7af0bab](https://github.com/pionxzh/wakaru/commit/7af0bab5f66e4004b3951f24c92478da54a40ca8)), closes [#81](https://github.com/pionxzh/wakaru/issues/81)

## [0.0.2](https://github.com/pionxzh/wakaru/compare/cli-v0.0.1...cli-v0.0.2) (2023-12-15)


### Features

* **cli:** generate `perf.json` when perf is enabled ([c5e59d6](https://github.com/pionxzh/wakaru/commit/c5e59d6d32d585b73f8da92adfc0d591bb50d24f))
* **cli:** implement new standalone CLI with better UX ([deda1df](https://github.com/pionxzh/wakaru/commit/deda1df1c2894c7e9b2b443c01033d366eec549c))
* **cli:** run unminify in worker ([9f38458](https://github.com/pionxzh/wakaru/commit/9f38458b198a48ad6fec95c7ede1c9ff41f283d8))
* **cli:** support performance statistic ([771b639](https://github.com/pionxzh/wakaru/commit/771b639f8147666dee984c5c196d746db5a896b3))
* support non-interative mode ([7f70b43](https://github.com/pionxzh/wakaru/commit/7f70b43f1c4365476c4a4ec2aa77a86652052a2d))
* use poolifier to manage worker ([59f5ddc](https://github.com/pionxzh/wakaru/commit/59f5ddc61d2b0e7f7b59beadcb8c57887777cbf4))


### Bug Fixes

* **cli:** move out the timing measure ([de53a94](https://github.com/pionxzh/wakaru/commit/de53a94219d0c54cdf1d3fc8ddfe5c31aaace618))
* fix elapsed time ([2e766a4](https://github.com/pionxzh/wakaru/commit/2e766a4daa0985585a947d1824bc4c269fca703d))
* improve error log ([df455eb](https://github.com/pionxzh/wakaru/commit/df455eb5fc4186d0d57d7ae5d676a8b45407ad64))
* passing module meta and module mapping to unminify ([551199e](https://github.com/pionxzh/wakaru/commit/551199e52ff10f8a21770b51023fc0d4f7db574f))
* update clack patch and remove spinner for unpacking ([d3b3da8](https://github.com/pionxzh/wakaru/commit/d3b3da871398062684da4d80cd372ffef706b421))
