# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1](https://github.com/andreiltd/componentize-qjs/compare/v0.4.0...v0.4.1) - 2026-06-18

### Features

- move oxc_resolver to host side ([#49](https://github.com/andreiltd/componentize-qjs/pull/49))

## [0.4.0](https://github.com/andreiltd/componentize-qjs/compare/v0.3.0...v0.4.0) - 2026-06-16

### Bug Fixes

- allow unknown imports during wizening ([#48](https://github.com/andreiltd/componentize-qjs/pull/48))
- *(deps)* update rust dependencies ([#43](https://github.com/andreiltd/componentize-qjs/pull/43))
- transfer owned WIT u8/s8 raw lists directly into TypedArray ([#46](https://github.com/andreiltd/componentize-qjs/pull/46))
- preserve `this` binding when slicing ([#40](https://github.com/andreiltd/componentize-qjs/pull/40))
- harden publish workflow

### Features

- [**breaking**] align top-level WIT result bindings with js throw semantics ([#47](https://github.com/andreiltd/componentize-qjs/pull/47))
- add build time module resolver ([#45](https://github.com/andreiltd/componentize-qjs/pull/45))
- [**breaking**] align bindings with jco ([#41](https://github.com/andreiltd/componentize-qjs/pull/41))

### Miscellaneous

- *(deps)* update github ci dependencies ([#44](https://github.com/andreiltd/componentize-qjs/pull/44))
- *(deps)* update npm dependencies ([#42](https://github.com/andreiltd/componentize-qjs/pull/42))

## [0.3.0](https://github.com/andreiltd/componentize-qjs/compare/v0.2.2...v0.3.0) - 2026-06-09

### Bug Fixes

- *(deps)* update rust dependencies to v45 ([#35](https://github.com/andreiltd/componentize-qjs/pull/35))
- *(deps)* update rust dependencies ([#34](https://github.com/andreiltd/componentize-qjs/pull/34))
- address regression in 1.96 toolchain ([#36](https://github.com/andreiltd/componentize-qjs/pull/36))
- *(ci)* do not persist credentials ([#30](https://github.com/andreiltd/componentize-qjs/pull/30))

### Features

- cache runtime builds ([#38](https://github.com/andreiltd/componentize-qjs/pull/38))
- publish runtime built without async component model ([#37](https://github.com/andreiltd/componentize-qjs/pull/37))

### Miscellaneous

- *(deps)* update npm dependencies ([#33](https://github.com/andreiltd/componentize-qjs/pull/33))
- *(deps)* update github ci dependencies ([#32](https://github.com/andreiltd/componentize-qjs/pull/32))

## [0.2.2](https://github.com/andreiltd/componentize-qjs/compare/v0.2.1...v0.2.2) - 2026-05-27

### Bug Fixes

- add cargo binstall metadata ([#29](https://github.com/andreiltd/componentize-qjs/pull/29))
- accept typed arrays in writable.write() ([#28](https://github.com/andreiltd/componentize-qjs/pull/28))
- *(deps)* update rust dependencies ([#27](https://github.com/andreiltd/componentize-qjs/pull/27))
- *(deps)* update rust dependencies ([#25](https://github.com/andreiltd/componentize-qjs/pull/25))

### Miscellaneous

- *(deps)* update github ci dependencies ([#24](https://github.com/andreiltd/componentize-qjs/pull/24))
- *(deps)* update dependency vitest to v4.1.6 ([#23](https://github.com/andreiltd/componentize-qjs/pull/23))

## [0.2.1](https://github.com/andreiltd/componentize-qjs/compare/v0.2.0...v0.2.1) - 2026-05-12

### Features

- allow runtime builds without async support ([#19](https://github.com/andreiltd/componentize-qjs/pull/19))

### Miscellaneous

- *(ci)* enable trusted publishing ([#22](https://github.com/andreiltd/componentize-qjs/pull/22))
- bump binaryen and wasi-sdk ([#20](https://github.com/andreiltd/componentize-qjs/pull/20))

## [0.2.0](https://github.com/andreiltd/componentize-qjs/compare/v0.1.0...v0.2.0) - 2026-05-05

### Bug Fixes

- *(ci)* fix release workflow permissions ([#17](https://github.com/andreiltd/componentize-qjs/pull/17))
- *(deps)* update rust dependencies ([#9](https://github.com/andreiltd/componentize-qjs/pull/9))
- *(deps)* update rust dependencies to v44 ([#14](https://github.com/andreiltd/componentize-qjs/pull/14))
- *(ci)* pin actions ([#6](https://github.com/andreiltd/componentize-qjs/pull/6))
- *(ci)* wire release asset publishing through release-plz ([#4](https://github.com/andreiltd/componentize-qjs/pull/4))

### Features

- add runtime optimized for size ([#15](https://github.com/andreiltd/componentize-qjs/pull/15))
- switch from JS script to ES modules ([#8](https://github.com/andreiltd/componentize-qjs/pull/8))

### Miscellaneous

- *(ci)* add npm publish job ([#16](https://github.com/andreiltd/componentize-qjs/pull/16))
- *(deps)* update dependency @napi-rs/cli to v3.6.2 ([#10](https://github.com/andreiltd/componentize-qjs/pull/10))
- *(deps)* update dependency node to v24 ([#12](https://github.com/andreiltd/componentize-qjs/pull/12))
- *(deps)* update github ci dependencies ([#11](https://github.com/andreiltd/componentize-qjs/pull/11))
- *(deps)* update dependency vitest to v4 ([#13](https://github.com/andreiltd/componentize-qjs/pull/13))
- *(ci)* add renovate bot config ([#7](https://github.com/andreiltd/componentize-qjs/pull/7))
