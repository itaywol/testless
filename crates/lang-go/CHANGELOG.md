# Changelog

## [0.6.0](https://github.com/itaywol/testless/compare/testless-lang-go-v0.5.1...testless-lang-go-v0.6.0) (2026-07-24)


### Bug Fixes

* **lang-go:** include import aliases in read known-names ([e8f27b9](https://github.com/itaywol/testless/commit/e8f27b998549f85203d4268a6020932401204f6c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.5.1 to 0.6.0

## [0.5.1](https://github.com/itaywol/testless/compare/testless-lang-go-v0.5.0...testless-lang-go-v0.5.1) (2026-07-24)


### Miscellaneous Chores

* release with package readmes ([ed14b4e](https://github.com/itaywol/testless/commit/ed14b4e6d7e7c03921d7fcb32b3ec822d80af858))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.5.0 to 0.5.1

## [0.5.0](https://github.com/itaywol/testless/compare/testless-lang-go-v0.4.0...testless-lang-go-v0.5.0) (2026-07-24)


### Miscellaneous Chores

* **testless-lang-go:** Synchronize testless versions


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.4.0 to 0.5.0

## [0.4.0](https://github.com/itaywol/testless/compare/testless-lang-go-v0.3.0...testless-lang-go-v0.4.0) (2026-07-23)


### Features

* **core:** def-level structural diff with sig/body split ([2766107](https://github.com/itaywol/testless/commit/27661076be468812f1bcca7934ec439832a15a35))


### Bug Fixes

* **lang-go:** init() body hashes into ModuleInit instead of being skipped ([31db7a0](https://github.com/itaywol/testless/commit/31db7a00224bb0e18ccc0c0459c616378b281775))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.3.0 to 0.4.0

## [0.3.0](https://github.com/itaywol/testless/compare/testless-lang-go-v0.2.0...testless-lang-go-v0.3.0) (2026-07-23)


### Features

* **core:** extraction model carries call/read references ([6339e65](https://github.com/itaywol/testless/commit/6339e6514c6a0c473e39033b646eb0fd6c9b0ce5))
* **lang-go:** call/read extraction; narrow t.Run to testing receivers ([ef583cc](https://github.com/itaywol/testless/commit/ef583cc120fd34101c90dd6a4ad825a80446a4de))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.2.0 to 0.3.0

## [0.2.0](https://github.com/itaywol/testless/compare/testless-lang-go-v0.1.0...testless-lang-go-v0.2.0) (2026-07-23)


### Features

* **lang-go:** extraction and module-internal import resolution ([6d5ff4a](https://github.com/itaywol/testless/commit/6d5ff4a630b903d2751992c05cf450de85505090))


### Bug Fixes

* explicit crate versions for release-please cargo-workspace plugin ([b6f206d](https://github.com/itaywol/testless/commit/b6f206d3636cf232c0af9d18c034c268d7372986))
* final-review batch: self-edge guard, cache version magic, test gaps ([a675735](https://github.com/itaywol/testless/commit/a675735f6a713d8bc7a48e1da70d889aa971d464))
* **lang-go:** thread enclosing subtest context through nested t.Run chains ([d1b8a71](https://github.com/itaywol/testless/commit/d1b8a71d9656030a7ff9da23620d67e5590b6974))
* publishable crate versions, strict publish loop, review nits ([01b86d9](https://github.com/itaywol/testless/commit/01b86d9fce3d20ba0da79417587b4469da005964))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.1.0 to 0.2.0
