# Changelog

## [0.5.1](https://github.com/itaywol/testless/compare/testless-core-v0.5.0...testless-core-v0.5.1) (2026-07-24)


### Miscellaneous Chores

* release with package readmes ([ed14b4e](https://github.com/itaywol/testless/commit/ed14b4e6d7e7c03921d7fcb32b3ec822d80af858))

## [0.5.0](https://github.com/itaywol/testless/compare/testless-core-v0.4.0...testless-core-v0.5.0) (2026-07-24)


### Features

* **cli:** select command: impacted-test selection output ([2a5a657](https://github.com/itaywol/testless/commit/2a5a657576a02a3a515291461a76666e5989ea24))
* **core:** reverse-reachability impact walk with widening rules ([dc94950](https://github.com/itaywol/testless/commit/dc94950613f6c6a69b7b360b790ddddabd89008f))


### Bug Fixes

* **core:** enqueue module-init-widened tests so they propagate ([4221758](https://github.com/itaywol/testless/commit/4221758fb453990bbdebdeb81be2e1e7f6cfa0d5))

## [0.4.0](https://github.com/itaywol/testless/compare/testless-core-v0.3.0...testless-core-v0.4.0) (2026-07-23)


### Features

* **cli:** changes command: classified impact seeds ([073011e](https://github.com/itaywol/testless/commit/073011eaa0050f7ffd30160f2684d2508b91dc9f))
* **core:** change classification to impact seeds ([d6f53b7](https://github.com/itaywol/testless/commit/d6f53b7678d179eb3a2909ce19d9128f98cb48d0))
* **core:** def-level structural diff with sig/body split ([2766107](https://github.com/itaywol/testless/commit/27661076be468812f1bcca7934ec439832a15a35))
* **core:** git changed-file listing and rev file content ([fd1967d](https://github.com/itaywol/testless/commit/fd1967da1d7c93ecaeadd49daa6432f62cfe11ff))
* **core:** structural fingerprints, comment/format-insensitive ([17aa8e5](https://github.com/itaywol/testless/commit/17aa8e5476b5e2daacc25b0ebb383cde0c4714b8))


### Bug Fixes

* TS module-scope change visibility, git-status widening, review nits ([eba2664](https://github.com/itaywol/testless/commit/eba266441a6b1e5260131597aa197a867fc3eb46))

## [0.3.0](https://github.com/itaywol/testless/compare/testless-core-v0.2.0...testless-core-v0.3.0) (2026-07-23)


### Features

* **core:** extraction model carries call/read references ([6339e65](https://github.com/itaywol/testless/commit/6339e6514c6a0c473e39033b646eb0fd6c9b0ce5))
* **core:** tier-1 call/read resolution with unknown widening markers ([980b560](https://github.com/itaywol/testless/commit/980b560047a76da0a142aa0bc910523bf60d5c52))


### Bug Fixes

* **core:** widen self-only-candidate call refs to Unknown instead of dropping ([80919fd](https://github.com/itaywol/testless/commit/80919fdb3621217843eab04f29a17df16d486303))


### Performance Improvements

* **core:** O(1) module_init and hashed file/name lookups ([c636af1](https://github.com/itaywol/testless/commit/c636af181b691cd7331395a7e7368dc16126ce8e))

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
* final-review batch: self-edge guard, cache version magic, test gaps ([a675735](https://github.com/itaywol/testless/commit/a675735f6a713d8bc7a48e1da70d889aa971d464))
* publishable crate versions, strict publish loop, review nits ([01b86d9](https://github.com/itaywol/testless/commit/01b86d9fce3d20ba0da79417587b4469da005964))
