# Changelog

## [1.5.0](https://github.com/TheUncharted/zapcode/compare/v1.4.0...v1.5.0) (2026-03-13)


### Features

* add event loop for async array callbacks (Promise.all, .map, .forEach) ([17ae149](https://github.com/TheUncharted/zapcode/commit/17ae1498ca9b1e290283dd78142fca15dda66f12))
* add event loop for async array callbacks (Promise.all, .map, .forEach) ([5e756a0](https://github.com/TheUncharted/zapcode/commit/5e756a09c2bfe64f9bb6e89ccb69cebf055e7ab0))

## [1.4.0](https://github.com/TheUncharted/zapcode/compare/v1.3.0...v1.4.0) (2026-03-12)


### Features

* add autoFix and execution trace to zapcode-ai packages ([124eec9](https://github.com/TheUncharted/zapcode/commit/124eec993cb0511e4163602e4da9c452e6998801))
* add execution trace system to zapcode-core ([dc545dc](https://github.com/TheUncharted/zapcode/commit/dc545dc657c7133f72aeef7a02cafd18a5086ba2))
* Add new feature related to debugging and tracing ([d3a37cc](https://github.com/TheUncharted/zapcode/commit/d3a37cc5dfef19026c26aba842321e3ca1bbc355))
* expose execution trace in JS, Python, and WASM bindings ([299c10c](https://github.com/TheUncharted/zapcode/commit/299c10c045e9c0dd64757c2f06de240dbd7ebe1b))


### Bug Fixes

* address CodeRabbit review findings ([cdbc6ee](https://github.com/TheUncharted/zapcode/commit/cdbc6eeed8357a3b4fbc4066d584e003c42ff143))
* update CI paths after examples directory reorganization ([ccfc2ee](https://github.com/TheUncharted/zapcode/commit/ccfc2ee8ee05e05d628b10211635a834a350f79d))

## [1.3.0](https://github.com/TheUncharted/zapcode/compare/v1.2.0...v1.3.0) (2026-03-12)


### Features

* Promise .then/.catch/.finally, VM bugfixes, release pipeline improvements ([b8f2c81](https://github.com/TheUncharted/zapcode/commit/b8f2c8118f3b3f447e76aa6774b6d278a911944b))

## [1.2.0](https://github.com/TheUncharted/zapcode/compare/v1.1.7...v1.2.0) (2026-03-12)


### Features

* add Promise .then(), .catch(), .finally() instance methods ([9cf33db](https://github.com/TheUncharted/zapcode/commit/9cf33db9bd767c6efb1540adbb0642041ad47d34))

## [1.1.7](https://github.com/TheUncharted/zapcode/compare/v1.1.6...v1.1.7) (2026-03-12)


### Bug Fixes

* switch Bedrock examples to global.amazon.nova-2-lite-v1:0 ([f6e5889](https://github.com/TheUncharted/zapcode/commit/f6e58891fc7798304f49f146355d77251e2134be))
* switch Bedrock examples to global.amazon.nova-2-lite-v1:0 ([b5d29cb](https://github.com/TheUncharted/zapcode/commit/b5d29cbbfc037b06afc2a17e10e9ff1504985a6f))

## [1.1.6](https://github.com/TheUncharted/zapcode/compare/v1.1.5...v1.1.6) (2026-03-12)


### Bug Fixes

* skip receiver writeback for builtin globals to prevent snapshot serialization error ([b41794a](https://github.com/TheUncharted/zapcode/commit/b41794a1b8c3a2a4e1f6ab8e500f7909504dd825))

## [1.1.5](https://github.com/TheUncharted/zapcode/compare/v1.1.4...v1.1.5) (2026-03-12)


### Bug Fixes

* persist this mutations back to receiver after method calls ([c1b657f](https://github.com/TheUncharted/zapcode/commit/c1b657fdc7658d335ca2c5d8e6c7c8d13098ed7a))
* use latest published versions in examples instead of local paths ([4afd4d9](https://github.com/TheUncharted/zapcode/commit/4afd4d92a2ca8faccce2fe2f5b3dbe2095fb026b))

## [1.1.4](https://github.com/TheUncharted/zapcode/compare/v1.1.3...v1.1.4) (2026-03-12)


### Bug Fixes

* strip trailing comment from version extraction in release workflow ([8bd63e1](https://github.com/TheUncharted/zapcode/commit/8bd63e11521246430f577eda42983d009ca7cc6e))
* strip trailing comment from version extraction in release workflow ([12ea165](https://github.com/TheUncharted/zapcode/commit/12ea165e6d201d40b5d6aa59cd456107073ca63e))

## [1.1.3](https://github.com/TheUncharted/zapcode/compare/v1.1.2...v1.1.3) (2026-03-12)


### Bug Fixes

* sync Python package versions to 1.1.2 to match npm and crates.io ([728deda](https://github.com/TheUncharted/zapcode/commit/728dedac9f22b05a538ab8e6ed4cd31403dda886))
* sync Python package versions to 1.1.2 to match npm and crates.io ([ba80dc4](https://github.com/TheUncharted/zapcode/commit/ba80dc425ad41f1987118472287cd68afecd83c8))

## [1.1.2](https://github.com/TheUncharted/zapcode/compare/v1.1.1...v1.1.2) (2026-03-12)


### Bug Fixes

* use absolute URL for logo so it renders on PyPI, npm, and crates.io ([56d2c53](https://github.com/TheUncharted/zapcode/commit/56d2c53579d565db846a1acee6e27bc4f4b73a89))
* use absolute URL for logo so it renders on PyPI, npm, and crates.io ([412308b](https://github.com/TheUncharted/zapcode/commit/412308baabd912caeaee9b8ab6c91cbee4cc8e8b))

## [1.1.1](https://github.com/TheUncharted/zapcode/compare/v1.1.0...v1.1.1) (2026-03-12)


### Bug Fixes

* add readme field to zapcode-ai pyproject.toml for PyPI description ([acef8a7](https://github.com/TheUncharted/zapcode/commit/acef8a79e11134d085c39bdf8b2a88e88879678a))
* add readme field to zapcode-ai pyproject.toml for PyPI description ([4d2db80](https://github.com/TheUncharted/zapcode/commit/4d2db802885b7c3fff1b43b459e410048b1dcc76))

## [1.1.0](https://github.com/TheUncharted/zapcode/compare/v1.0.1...v1.1.0) (2026-03-12)


### Features

* add CI/CD pipelines and update tagline ([886a9dd](https://github.com/TheUncharted/zapcode/commit/886a9ddc1cce965cd977ba2f6fef75fd4f5bda87))
* add multi-platform npm and PyPI publishing ([c148f49](https://github.com/TheUncharted/zapcode/commit/c148f49b07d7988bbed1a8f8060fb6e40c6e7438))
* add project logo to README ([0c62538](https://github.com/TheUncharted/zapcode/commit/0c625385b2641ed658347de1dd3e5c3817d69751))
* add zapcode-ai Python wrapper to release workflow ([76653b3](https://github.com/TheUncharted/zapcode/commit/76653b3f7f1f779dd5c6fd788be9b609a3163085))
* add zapcode-ai to release workflow ([4430a7b](https://github.com/TheUncharted/zapcode/commit/4430a7baa6fdaa666cd6a77c20771e766c9df6ce))
* auto-detect trailing object literals without parentheses ([f0934ef](https://github.com/TheUncharted/zapcode/commit/f0934ef9e6fd60595dd30a2c24967d2d23a9c9ca))
* **ci:** post benchmark results as PR comment ([c3c6256](https://github.com/TheUncharted/zapcode/commit/c3c6256bc54e2defb114771a36bfe58a156800c9))
* enable Release Please for automated versioning ([37284bb](https://github.com/TheUncharted/zapcode/commit/37284bbecb7a5fcd08c648d05f98f83dbb790135))
* show version in release workflow run name ([014f096](https://github.com/TheUncharted/zapcode/commit/014f096f118dabd617ce627488f359fb19cd3327))


### Bug Fixes

* add --platform flag to napi build in release workflow ([2633f95](https://github.com/TheUncharted/zapcode/commit/2633f9568537dc901e8f829172b7bfd77f9d7f24))
* add 10MB size limit to string concatenation ([662dddc](https://github.com/TheUncharted/zapcode/commit/662dddc2975da629971b9e3a871c46286288102c))
* add base_branches to CodeRabbit config for master and develop ([6410155](https://github.com/TheUncharted/zapcode/commit/64101557297d611169fd8623cfc8ff03ac989c9d))
* add base_branches to CodeRabbit config for master and develop ([846aa33](https://github.com/TheUncharted/zapcode/commit/846aa336404930edb1f6d54c351454cf249ac421))
* add size limit to string.repeat to prevent OOM ([52f062d](https://github.com/TheUncharted/zapcode/commit/52f062d928a29169b94dc7beb3c8aad660e8ead5))
* build PyPI wheels for Python 3.10-3.13 on all platforms ([38a7e21](https://github.com/TheUncharted/zapcode/commit/38a7e21eca3bdc31dbec7e28d7111a024cf6bede))
* **ci:** await createComment and update existing benchmark comment ([5165981](https://github.com/TheUncharted/zapcode/commit/5165981f80af238b95097508e0a9dd768c19213d))
* **ci:** copy README.md before maturin build in e2e-python ([f998a04](https://github.com/TheUncharted/zapcode/commit/f998a042a27dc7e2f84832046564df27b2b679fa))
* **ci:** strip ANSI colors from benchmark output ([228f47a](https://github.com/TheUncharted/zapcode/commit/228f47aa2441a018b42b014faec2f75e928a0dc9))
* copy built JS bindings into example node_modules ([a2e49f8](https://github.com/TheUncharted/zapcode/commit/a2e49f87925befcefe7d281ad694c203ecf51483))
* count total tool calls instead of steps with tool calls ([1d6750d](https://github.com/TheUncharted/zapcode/commit/1d6750d254864883c810f9cb98506468b1af77f3))
* create npm platform dirs before moving artifacts ([fa07125](https://github.com/TheUncharted/zapcode/commit/fa07125f9d5abc1d88b97d4395d1f343ce6998ec))
* cross-compilation in release workflow ([8fde562](https://github.com/TheUncharted/zapcode/commit/8fde562bdaac66ecdeab0fce2d1e1006484b48f6))
* disable component in tag for release-please ([b0cc762](https://github.com/TheUncharted/zapcode/commit/b0cc762a48482bf9ea137de3c5f140c6989afa66))
* fail explicitly when step budget is exhausted in Python example ([88739fc](https://github.com/TheUncharted/zapcode/commit/88739fc6c32ce323f83d44a4f5c5a6469714b3c1))
* handle unexpected stop reasons in Bedrock Converse response ([57ed788](https://github.com/TheUncharted/zapcode/commit/57ed788c0b15f757e5877ceba6cf0012ec16c306))
* improve AI examples and system prompt for better DX ([2e7f854](https://github.com/TheUncharted/zapcode/commit/2e7f854b4a4e52f7e1a9a22687208d343dc135e8))
* improve error handling for unsupported statement types in AST lowering ([00ff719](https://github.com/TheUncharted/zapcode/commit/00ff719cefc70fdc3354a2704389fe7b3c96b64b))
* include route in mock flight search results ([e501fc8](https://github.com/TheUncharted/zapcode/commit/e501fc8443925ff7aa01e530b7b7994800576b00))
* make release workflow idempotent for re-runs ([f693d90](https://github.com/TheUncharted/zapcode/commit/f693d90db9b17d8b61bc8215d9dabbc1bdefbacd))
* move dependencies out of project.urls in zapcode-ai-python ([1344644](https://github.com/TheUncharted/zapcode/commit/1344644b5181bdf8a19462953802140a3a633018))
* move readme to [tool.maturin] section for PyPI ([e8d8b93](https://github.com/TheUncharted/zapcode/commit/e8d8b93f6fa71ac8c35142162c64f749cb20a4c2))
* release-please config and zapcode-ai publishing ([5423a52](https://github.com/TheUncharted/zapcode/commit/5423a52cdc1784d0491e8708729c68e2cd0cb9c4))
* remove release-type override from workflow to use config file ([6d596f6](https://github.com/TheUncharted/zapcode/commit/6d596f65022a1213a1ef45877ad436bf0c129cd8))
* remove release-type override from workflow to use config file ([401b216](https://github.com/TheUncharted/zapcode/commit/401b2161b8660e87b16ef09bd04036ea77166bf9))
* resolve CI failures — formatting, clippy, and lockfile ([e7899f1](https://github.com/TheUncharted/zapcode/commit/e7899f10dd82ba6ad328bcfadfe637d6972731d8))
* resolve CI test failure and security audit ([24f15db](https://github.com/TheUncharted/zapcode/commit/24f15dbc6e4f57551110146eaffa439632a82574))
* resolve E2E failures — ESM exports and Python venv ([1eb5ec3](https://github.com/TheUncharted/zapcode/commit/1eb5ec33bc308cfddc23c6fe51fd6c53a4b5694d))
* soften system prompt rule about structured object access ([3202a84](https://github.com/TheUncharted/zapcode/commit/3202a84e325a0154c9e549a4e4cb4d75e9abe1cf))
* strengthen if/else test assertion, check it's not wrapped as object ([1461cda](https://github.com/TheUncharted/zapcode/commit/1461cdaddcafeebca85b42f44e72eaaa20e730cc))
* switch release-please to simple strategy for cargo workspace compat ([a66783c](https://github.com/TheUncharted/zapcode/commit/a66783ccfe74c51b18d37d7811c8804af65a94f7))
* switch release-please to simple strategy for cargo workspace compat ([d9a5cd1](https://github.com/TheUncharted/zapcode/commit/d9a5cd18ffde49d893cc164a626868f73b71b4b8))
* tighten object literal heuristic, disallow dots in shorthand check ([6ab93d2](https://github.com/TheUncharted/zapcode/commit/6ab93d2a9a863937e82b29636b83f0f61e83697c))
* track index.js and index.d.ts in git for JS bindings ([02fb350](https://github.com/TheUncharted/zapcode/commit/02fb3507c4b170881e27018700bdfc937c41e97c))
* use --output-dir for napi artifacts command ([74afb17](https://github.com/TheUncharted/zapcode/commit/74afb17c499d86f623ffdc71daf4f73fb22f2df0))
* use actual span in unsupported statement error instead of "unknown" ([1e26cd3](https://github.com/TheUncharted/zapcode/commit/1e26cd32a5871093705bfda1846ff58f4c5408ea))
* use AI SDK tool() + jsonSchema() for Vercel AI integration ([efdf8ad](https://github.com/TheUncharted/zapcode/commit/efdf8add1e3c18451d68a984cf99caa04b1ef382))
* use maturin-action interpreter input instead of -i flag ([6b099f8](https://github.com/TheUncharted/zapcode/commit/6b099f8c5c194fccc1cfa2c99af0c3a67407651b))
* zapcode-ai-python pyproject.toml and release-please config ([c889cd8](https://github.com/TheUncharted/zapcode/commit/c889cd8ce1d456848664810135560eba5a27ca9e))
