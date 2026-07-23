# Changelog

## [0.2.0](https://github.com/itaywol/testless/compare/testless-v0.1.0...testless-v0.2.0) (2026-07-23)


### Features

* **cli:** index and stats commands ([f17c9d9](https://github.com/itaywol/testless/commit/f17c9d92fa08156caba9e6a8c08cc33852de63eb))
* **cli:** register Rust language; dogfood self-index test ([825de3b](https://github.com/itaywol/testless/commit/825de3be7e14ba04c7f5d60547a803718354dc04))
* **cli:** shell completions and help examples ([9041839](https://github.com/itaywol/testless/commit/9041839e4e476a46a2b6bd3e561160d0d9eef505))
* **core:** cache with hash-gated incremental reindex ([b2cccd8](https://github.com/itaywol/testless/commit/b2cccd8f95489be9b74e16261d67be6838320397))
* **core:** repo indexer producing graph ([bef1f13](https://github.com/itaywol/testless/commit/bef1f13238b8a6ace28fa889f6ff9986960a92f6))


### Bug Fixes

* **cli:** expose Cache::file() and dedupe test-count logic ([03d6fb8](https://github.com/itaywol/testless/commit/03d6fb82235634e46812d677e5a88cae314f56f0))
* **core:** dedup Imports edges and guard Go dir fan-out self-edges ([c11221f](https://github.com/itaywol/testless/commit/c11221f022cdc2a1990615f2ed10841a0984ef74))
* explicit crate versions for release-please cargo-workspace plugin ([b6f206d](https://github.com/itaywol/testless/commit/b6f206d3636cf232c0af9d18c034c268d7372986))
* publishable crate versions, strict publish loop, review nits ([01b86d9](https://github.com/itaywol/testless/commit/01b86d9fce3d20ba0da79417587b4469da005964))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * testless-core bumped from 0.1.0 to 0.2.0
    * testless-lang-ts bumped from 0.1.0 to 0.2.0
    * testless-lang-go bumped from 0.1.0 to 0.2.0
    * testless-lang-rust bumped from 0.1.0 to 0.2.0
