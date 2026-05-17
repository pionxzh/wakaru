# Changelog

## [1.1.0](https://github.com/pionxzh/wakaru/compare/v1.0.0...v1.1.0) (2026-05-18)


### Features

* **core:** restore Babel regeneratorRuntime generators and _asyncToGenerator ([e249fc0](https://github.com/pionxzh/wakaru/commit/e249fc0e15734d5b9d9851c3cf8d18fe835d84d7))
* **core:** synthesize import/export for esbuild scope-hoisted modules ([043997d](https://github.com/pionxzh/wakaru/commit/043997d0bc58758c4cc81f46bbff038056b58c2e))
* **core:** handle Babel 7.24+ _callSuper and 1-arg _createClass in un_es6_class ([3813d79](https://github.com/pionxzh/wakaru/commit/3813d793d45843712e987ecd9337c64693e3b680))
* **core:** add opt-in diagnostics reporting ([7597c18](https://github.com/pionxzh/wakaru/commit/7597c1861be7f6e2e3e6eb957f8b95ed053f138e))
* **core:** add TDZ violation diagnostics ([0abcc05](https://github.com/pionxzh/wakaru/commit/0abcc05e325ec9acab38a8981d87db92359a8487))
* **core:** surface structured warnings from best-effort unpack ([34b3166](https://github.com/pionxzh/wakaru/commit/34b3166c46220ad92739674f8a3ecd8716c2a2dd))
* **core:** extend UnBracketNotation to cover prop names and assignment targets ([66cb3d7](https://github.com/pionxzh/wakaru/commit/66cb3d72e97df8e055a42b272351a76feb4dfe32))
* recover component names from Sentry data-sentry-component attrs ([ff8ad1e](https://github.com/pionxzh/wakaru/commit/ff8ad1e3506422c962cd932c50121279b01bfb19))
* add MatchContext for binding-aware babel helper matching ([e19480d](https://github.com/pionxzh/wakaru/commit/e19480d999413fa38dd88a8ea0e9ea922dea0431))
* hoist embedded require() calls in un_esm pre-pass ([087f9b8](https://github.com/pionxzh/wakaru/commit/087f9b852dcb5ad2b4b2712d618d0973c3074d40))


### Bug Fixes

* **unpack:** merge esbuild init factories into their target module ([06fcbdd](https://github.com/pionxzh/wakaru/commit/06fcbdd4276e280454f89734d1825d59713ab9e2))
* **core:** synthesize imports in esbuild factory modules for scope-hoisted refs ([26879c3](https://github.com/pionxzh/wakaru/commit/26879c31ffaef764170d0bc00026759b0a028757))
* **core:** add ImportDedup to standard pipeline to merge duplicate imports ([3c7426a](https://github.com/pionxzh/wakaru/commit/3c7426aa509859d3768b1b967e7088cbd6a5f7c0))
* **core:** preserve function expression statements in SimplifySequence ([fc01cb4](https://github.com/pionxzh/wakaru/commit/fc01cb436ca7ad1f3674682928005cbd692f916b))
* **core:** reorder DCE passes so DeadDecls runs before DeadImports ([65cbb4e](https://github.com/pionxzh/wakaru/commit/65cbb4ec1c8ec63fddd7244b4c876c1ce415cfac))
* **core:** fix var→const regression and resolve chained export renames ([f938306](https://github.com/pionxzh/wakaru/commit/f938306d1fd711c66e66dae05925d0eeb5a36abc))
* **core:** relocate pre-references after class in un_prototype_class ([fea73a7](https://github.com/pionxzh/wakaru/commit/fea73a796d3b5466e9be998b982dec701bac9a55))
* **core:** prevent smart_rename→arguments/eval and un_prototype_class TDZ ([e877ee7](https://github.com/pionxzh/wakaru/commit/e877ee73f775b1462dc1257cb512038d07d18da6))
* **core:** prevent var→let/const conversion when var hoisting is relied upon ([4fa95e0](https://github.com/pionxzh/wakaru/commit/4fa95e0fdb93604970ebf407c189ec19adf77e19))
* **core:** thread unresolved mark through optional call recovery ([a5b0f1a](https://github.com/pionxzh/wakaru/commit/a5b0f1a286c76922d33bd5d1d1663bbe054cb50d))
* **core:** thread unresolved_mark into smart_rename and un_es6_class ([4aa98b8](https://github.com/pionxzh/wakaru/commit/4aa98b8904f4c93f6ff72e44814c2ea9ca8f6088))
* **core:** thread unresolved_mark into un_then_catch, un_undefined_init, un_destructuring ([fe18f52](https://github.com/pionxzh/wakaru/commit/fe18f526d302f66b0ddfe4954b00cea1af15ff3a))
* **core:** trace skipped remove void pass ([e3b26ec](https://github.com/pionxzh/wakaru/commit/e3b26ec2c606c82252324245723b3ac36cc29cc6))
* **core:** scope named function arrow checks ([e842701](https://github.com/pionxzh/wakaru/commit/e8427016c0434cc7c80bb2300e0385bab42fa912))
* **core:** key temp inlining by binding context ([b2da6c9](https://github.com/pionxzh/wakaru/commit/b2da6c998ed940305dac1e8d30c0e6598890517b))
* **core:** compare expression identifiers by context ([4e033c1](https://github.com/pionxzh/wakaru/commit/4e033c1e007cd809d0e117931d891533e65a1c48))
* **core:** preserve lexical this in nested arrows ([c4b251a](https://github.com/pionxzh/wakaru/commit/c4b251a5ebebd044ad06178a9f29c6348c5ee874))
* **core:** avoid invalid JSX attr renames ([075e06e](https://github.com/pionxzh/wakaru/commit/075e06e2545e1a976246a4a8ba24d77689237ae7))
* **core:** preserve unparseable extracted modules ([d25069f](https://github.com/pionxzh/wakaru/commit/d25069fc3b5ca74b165a48e691d1e50d4e3adb0b))
* **core:** trust successful parser recovery ([87bb93d](https://github.com/pionxzh/wakaru/commit/87bb93d0d0e43d1f6a6aeb1c06d841b9c2957086))
* **core:** distinguish bundle parse failures ([49f9048](https://github.com/pionxzh/wakaru/commit/49f904873629c47b0edfba59771e0f39482a8829))
* **core:** bound synthetic name search ([06785fd](https://github.com/pionxzh/wakaru/commit/06785fd160052e87b2e6fb482f8b296d208f159c))
* **core:** avoid panic in prototype class builder ([33f8867](https://github.com/pionxzh/wakaru/commit/33f8867cfc70354dbab3f95e274f744d3e9c8e2b))
* **core:** propagate unpack module errors ([455706a](https://github.com/pionxzh/wakaru/commit/455706aa3f07b4c1ed4460476a4de6e561ddc3fa))
* **core:** expose second parameters pass in tracing ([5b6d8cc](https://github.com/pionxzh/wakaru/commit/5b6d8cc5e79b14b490300c8323cff3bb6a367da3))
* **core:** harden scope-hoist reference analysis ([98d7f42](https://github.com/pionxzh/wakaru/commit/98d7f4268b018ab729a1457c77d67593ff16630a))
* **core:** recover safe arg rest loop patterns ([5fbf7f7](https://github.com/pionxzh/wakaru/commit/5fbf7f7badcac5aa35774cbb6fc565f252b85775))
* **core:** preserve duplicate var bindings ([a664d82](https://github.com/pionxzh/wakaru/commit/a664d8288fc434062881965677b267a163b9a211))
* **core:** preserve argument copy loops in arg_rest ([368235b](https://github.com/pionxzh/wakaru/commit/368235bdee295ed27c9f04f726878be11032d028))
* rewrite numeric require() calls in webpack5 unpacker ([7e5770f](https://github.com/pionxzh/wakaru/commit/7e5770f5461368e9c5a07beb8005f65816a9ad40))


### Performance

* add release profile with fat LTO ([c503ba1](https://github.com/pionxzh/wakaru/commit/c503ba169e5efbd648f4da9d147d995af1ab30dc))
