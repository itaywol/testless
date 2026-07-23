# Changelog

## [0.2.0](https://github.com/itaywol/testless/compare/testless-core-v0.1.0...testless-core-v0.2.0) (2026-07-23)


### Features

* **core:** cache with hash-gated incremental reindex ([b2cccd8](https://github.com/itaywol/testless/commit/b2cccd8f95489be9b74e16261d67be6838320397))
* **core:** gitignore-aware file discovery ([436d608](https://github.com/itaywol/testless/commit/436d6081c4d2ee123781c4c8dcb98ae956f5d853))
* **core:** graph types with bincode roundtrip ([f34df53](https://github.com/itaywol/testless/commit/f34df5368050183c9ccf0f0610bcb5fa60c8f601))
* **core:** Language trait and registry ([d943069](https://github.com/itaywol/testless/commit/d943069a2bdfd72edda1c4c9d07882bd56e76067))
* **core:** repo indexer producing graph ([bef1f13](https://github.com/itaywol/testless/commit/bef1f13238b8a6ace28fa889f6ff9986960a92f6))


### Bug Fixes

* **cli:** expose Cache::file() and dedupe test-count logic ([03d6fb8](https://github.com/itaywol/testless/commit/03d6fb82235634e46812d677e5a88cae314f56f0))
* **core:** dedup Imports edges and guard Go dir fan-out self-edges ([c11221f](https://github.com/itaywol/testless/commit/c11221f022cdc2a1990615f2ed10841a0984ef74))
* **core:** test sort determinism, tighten strip_prefix, drop stale comment ([c692775](https://github.com/itaywol/testless/commit/c692775594bfb2369dce9ffce28ed52737c10dc2))
* explicit crate versions for release-please cargo-workspace plugin ([b6f206d](https://github.com/itaywol/testless/commit/b6f206d3636cf232c0af9d18c034c268d7372986))
* final-review batch — self-edge guard, cache version magic, test gaps ([a675735](https://github.com/itaywol/testless/commit/a675735f6a713d8bc7a48e1da70d889aa971d464))
* publishable crate versions, strict publish loop, review nits ([01b86d9](https://github.com/itaywol/testless/commit/01b86d9fce3d20ba0da79417587b4469da005964))
