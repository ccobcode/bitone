VERSION   ?= 0.2.0
DIST_DIR  := dist/bitone
DIST_NAME := bitone-v$(VERSION)-$(shell uname -s | tr '[:upper:]' '[:lower:]')-$(shell uname -m)

# Homebrew 的 cargo 排在 PATH 前面时会绕过 rust-toolchain.toml,这里固定用 rustup 的工具链.
# macOS 自带的 make 执行简单命令时不看改过的 PATH,所以都写完整路径.
RUST_BIN     := $(shell rustup which cargo | xargs dirname)
CARGO        := PATH=$(RUST_BIN):$(HOME)/.cargo/bin:$$PATH cargo
WASM_BINDGEN := $(HOME)/.cargo/bin/wasm-bindgen

LLVM         ?= /opt/homebrew/opt/llvm/bin
ANDROID_HOME ?= $(HOME)/Library/Android/sdk
JAVA_HOME    ?= $(shell /usr/libexec/java_home -v 21)
GRADLE       := cd apps/mobile && JAVA_HOME=$(JAVA_HOME) ./gradlew --console=plain

PRETTIER ?= apps/web/node_modules/.bin/prettier
WASM_OUT := apps/web/src/core/pkg
MOBILE   := apps/mobile/androidApp/src/main
# 有本地配置时优先用它,里面可以放 API key,不进仓库.
GATEWAY  ?= $(if $(wildcard gateway.local.toml),gateway.local.toml,crates/bitone-gateway/gateway.example.toml)

# iOS 只能在 macOS 上编译.
IOS_CHECK := $(if $(filter Darwin,$(shell uname -s)),:shared:compileKotlinIosSimulatorArm64)

# 去掉本机绝对路径,否则换一台机器构建出来的 .so 不一样.
REPRO_FLAGS := --remap-path-prefix=$(CURDIR)=/src --remap-path-prefix=$(HOME)/.cargo=/cargo \
	--remap-path-prefix=$(HOME)/.rustup=/rustup

.PHONY: dev gateway dist dist-server dist-android clean \
	test test-rust test-web test-android test-punct check-punct fix-punct test-copy check-copy \
	format format-check lint audit hooks \
	wasm android-core check-deps-wasm check-deps-android corpus protogen

# Development: 网关和网页端都在宿主机,网页端用 Vite 把 /v1 代理到网关
dev: apps/web/node_modules wasm
	@trap 'kill 0' EXIT; $(CARGO) run -p bitone-gateway -- --config $(GATEWAY) & pnpm -C apps/web dev

gateway:
	$(CARGO) run -p bitone-gateway -- --config $(GATEWAY)

# Distribution: 网关和网页端按平台打包,APK 单独产出.Linux arm64 上没有 NDK,所以两者分开.
# APK 未签名,发布后在本地签名,签名私钥不进 CI.
dist: dist-server dist-android

dist-server: apps/web/node_modules wasm
	@rm -rf dist
	@mkdir -p $(DIST_DIR)/bin
	@echo "[*] compiling gateway..."
	@$(CARGO) build --release -p bitone-gateway
	@echo "[*] building web..."
	@pnpm -C apps/web build
	@cp target/release/bitone-gateway $(DIST_DIR)/bin/
	@cp -R apps/web/dist $(DIST_DIR)/web
	@cp crates/bitone-gateway/gateway.example.toml LICENSE $(DIST_DIR)/
	@printf '%s\n' "$(VERSION)" > $(DIST_DIR)/VERSION
	@tar -C dist -czf dist/$(DIST_NAME).tar.gz bitone
	@rm -rf $(DIST_DIR)
	@echo "[+] package ready: dist/$(DIST_NAME).tar.gz"

dist-android: android-core
	@echo "[*] building cold wallet..."
	@$(GRADLE) :androidApp:assembleRelease -PappVersion=$(VERSION)
	@mkdir -p dist
	@cp apps/mobile/androidApp/build/outputs/apk/release/*.apk dist/bitone-v$(VERSION)-android-unsigned.apk
	@echo "[+] package ready: dist/bitone-v$(VERSION)-android-unsigned.apk"

clean:
	rm -rf dist apps/web/dist apps/mobile/build apps/mobile/*/build
	$(CARGO) clean

# Test & Quality
test: test-rust test-web test-android
	$(MAKE) test-punct check-punct test-copy check-copy format-check lint

test-rust:
	$(CARGO) test --workspace

test-web: apps/web/node_modules wasm
	pnpm -C apps/web exec tsc --noEmit
	pnpm -C apps/web test
	pnpm -C apps/web build

test-android: android-core
	$(GRADLE) :androidApp:assembleDebug :androidApp:assembleRelease :shared:testAndroidHostTest :shared:jvmTest \
		:androidApp:testDebugUnitTest $(IOS_CHECK)

test-punct:
	node --test scripts/fullwidth.test.mjs

check-punct:
	node scripts/fullwidth.mjs

fix-punct:
	node scripts/fullwidth.mjs --fix

test-copy:
	node --test scripts/copy-check.test.mjs

check-copy:
	node scripts/copy-check.mjs
	node scripts/i18n-check.mjs

format: apps/web/node_modules
	$(CARGO) fmt --all
	$(PRETTIER) --write .
	$(GRADLE) spotlessApply

format-check: apps/web/node_modules
	$(CARGO) fmt --all --check
	$(PRETTIER) --check .
	$(GRADLE) spotlessCheck

lint: apps/web/node_modules
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	pnpm -C apps/web exec biome lint

# crypto-bigint 0.5 只经 alloy 关掉的可选特性 k256 进入锁文件,不编译,所以 RUSTSEC-2026-0387 不影响产物.
audit:
	$(CARGO) audit --ignore RUSTSEC-2026-0387
	$(CARGO) deny check
	$(CARGO) deny --manifest-path tools/crosscheck/deps/Cargo.toml check licenses bans sources
	$(CARGO) deny --manifest-path tools/protogen/Cargo.toml check licenses bans sources

hooks:
	git config core.hooksPath .githooks

# 生成物
wasm:
	CC_wasm32_unknown_unknown=$(LLVM)/clang AR_wasm32_unknown_unknown=$(LLVM)/llvm-ar \
		$(CARGO) build -p bitone-wasm --target wasm32-unknown-unknown --release
	$(WASM_BINDGEN) target/wasm32-unknown-unknown/release/bitone_wasm.wasm --target web --out-dir $(WASM_OUT)

android-core:
	RUSTFLAGS="$(REPRO_FLAGS)" ANDROID_NDK_HOME=$(ANDROID_HOME)/ndk/29.0.14206865 \
		$(CARGO) ndk -t arm64-v8a -t x86_64 -P 31 -o $(MOBILE)/jniLibs build -p bitone-ffi --release
	$(CARGO) run -p bitone-ffi --bin uniffi-bindgen -- generate --library \
		target/aarch64-linux-android/release/libbitone_ffi.so --language kotlin --out-dir $(MOBILE)/java

# 标准列出的运行时依赖还没进产物,先单独验证能在 wasm 和 Android 上构建.
check-deps-wasm:
	cd tools/crosscheck/deps && CC_wasm32_unknown_unknown=$(LLVM)/clang AR_wasm32_unknown_unknown=$(LLVM)/llvm-ar \
		$(CARGO) build --locked --target wasm32-unknown-unknown

check-deps-android:
	cd tools/crosscheck/deps && ANDROID_NDK_HOME=$(ANDROID_HOME)/ndk/29.0.14206865 \
		$(CARGO) ndk -t arm64-v8a -P 31 build --locked

# 对照工具和生成器有自己的工具链,所以在它的目录里取 cargo.
corpus:
	cd tools/protogen && PATH=$$(rustup which cargo | xargs dirname):$$PATH cargo run --locked -- --check
	cd tools/crosscheck && PATH=$$(rustup which cargo | xargs dirname):$$PATH cargo test --locked
	cd tools/crosscheck && PATH=$$(rustup which cargo | xargs dirname):$$PATH cargo run --locked

# 换 schemas/ 之后重新生成 TRON 的类型和字段表.
protogen:
	cd tools/protogen && PATH=$$(rustup which cargo | xargs dirname):$$PATH cargo run --locked

apps/web/node_modules: apps/web/package.json apps/web/pnpm-lock.yaml
	pnpm -C apps/web install --frozen-lockfile
	@touch $@
