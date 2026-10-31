<p align="center">
  <img src="apps/web/public/favicon.svg" width="112" alt="BitOne">
</p>

<h1 align="center">BitOne</h1>

<p align="center">
  <b>两端之间，只有一个比特。</b><br>
  私钥离线 | 二维码传输 | 多链支持
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.95-000000?logo=rust&logoColor=white" alt="Rust 1.95">
  <img src="https://img.shields.io/badge/React-19-149ECA?logo=react&logoColor=white" alt="React 19">
  <img src="https://img.shields.io/badge/Kotlin-2.4-7F52FF?logo=kotlin&logoColor=white" alt="Kotlin 2.4">
  <img src="https://img.shields.io/badge/Android-12+-3DDC84?logo=android&logoColor=white" alt="Android 12+">
</p>

<p align="center">
  <a href="#架构">架构</a> |
  <a href="#功能">功能</a> |
  <a href="#快速开始">快速开始</a> |
  <a href="#开发">开发</a> |
  <a href="#构建">构建</a> |
  <a href="docs/README.md">文档</a>
</p>

---

BitOne 是一款多链冷钱包。私钥只在离线的 Android 手机上，网页端在浏览器里构建交易，两者只通过二维码交换数据。

## 架构

- **冷钱包**：Android 应用，保管密钥，核对并签名。不申请联网权限。
- **网页端**：构建交易、管理观察钱包。只有公钥，数据存在浏览器里。
- **网关**：提供查询、WebSocket 订阅和代理广播。不存用户数据，不鉴权。

## 功能

- **多链**：Bitcoin、Ethereum、BNB Smart Chain、Polygon、TRON、Solana 和它们的测试网。
- **两层加密**：种子先用 PIN 派生的密钥加密，再用 Keystore 硬件密钥加密。
- **气隙传输**：BC-UR 动态二维码，大交易分帧循环播放。
- **交易核对**：冷钱包自己解析交易并判定风险，不信任网页端的说明。
- **内置清单**：代币信息随应用内置，扫码离线更新，验签后才生效。

## 快速开始

### 1. 自部署

```bash
docker build -f docker/selfhost.Dockerfile -t bitone .

# 改两行：listen = "0.0.0.0:8787"，web_dir = "/srv/bitone/web"
cp crates/bitone-gateway/gateway.example.toml gateway.toml

docker run -p 8787:8787 -v ./gateway.toml:/etc/bitone/gateway.toml bitone
```

打开 `http://localhost:8787`。

示例配置用的是免费公共节点，有频率限制。正式部署请换成自己的节点或带 key 的服务。

### 2. 从源码运行

需要 rustup、Node、pnpm、JDK 21、Android SDK 和 NDK。WASM 构建需要 Homebrew 的 LLVM（`brew install llvm`）。

```bash
# 启动网关和网页端，打开 http://localhost:5173
make dev
```

`make dev` 优先读根目录的 `gateway.local.toml`，没有时读示例配置。要填 API key 时复制一份再改，这个文件不进 Git。

```bash
cp crates/bitone-gateway/gateway.example.toml gateway.local.toml
```

### 3. 安装冷钱包

手机连上数据线，开启 USB 调试。

```bash
make android-core
cd apps/mobile && ./gradlew :androidApp:assembleDebug
adb install -r androidApp/build/outputs/apk/debug/androidApp-debug.apk
```

## 开发

```bash
# 跑全部测试和检查
make test

# 格式化 Rust、前端和 Kotlin
make format

# 修改 design/tokens.json 后重新生成两端的令牌
make tokens
```

`make lint` 跑 clippy 和 Biome，`make audit` 审查依赖漏洞和许可证，`make hooks` 启用 Git 钩子。

## 构建

```bash
make dist VERSION=0.1.0
```

产物在 `dist/`：网关加网页端的 `bitone-v<VERSION>-<os>-<arch>.tar.gz`，和未签名的 `bitone-v<VERSION>-android-unsigned.apk`。

推送 `v1.2.3` 这样的 tag 后，CI 通过就自动发布 GitHub Release。版本号要先在 `Cargo.toml` 里改好。APK 的签名私钥不进 CI，发布后在离线机器上签名再上传。
