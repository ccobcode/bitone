# 技术方案

依据 [系统设计](03-system.md) 和 [安全设计](04-security.md) 做的选型。实际版本以锁文件为准。

## 1. 选型结论

| 部分         | 选型                                                           | 版本                    |
| ------------ | -------------------------------------------------------------- | ----------------------- |
| 核心库       | Rust，密钥、解析、协议都在这里                                 | Rust 1.95               |
| 冷钱包界面   | Kotlin + Compose Multiplatform，只用 Foundation 层，组件自己写 | Kotlin 2.4，CMP 1.12    |
| Android 构建 | AGP 9.3、Gradle 9.8、minSdk 31、targetSdk 36、compileSdk 37    |                         |
| 核心库绑定   | UniFFI（Android、iOS），wasm-bindgen（网页）                   | UniFFI 0.32             |
| 网页端       | React + Vite + TypeScript                                      | React 19，Vite 8        |
| 网页 UI 库   | Base UI 无样式组件 + Tailwind CSS + 自己的组件层               | Base UI 1.9，Tailwind 4 |
| 网页数据     | Dexie（IndexedDB）、TanStack Query                             | Dexie 4，Query 5        |
| 网关         | Rust，axum + tokio                                             | axum 0.8                |
| 设计令牌     | `design/tokens.json` 生成 CSS 变量和 Kotlin 主题               |                         |

## 2. 选型理由

### 2.1 所有敏感逻辑只写一份，用 Rust

助记词、派生、签名、交易解析、风险分类、二维码协议都放进 Rust 核心库，冷钱包和网页端共用。

- **日后加 iOS 不用重写安全逻辑**：只有一份解析和分类代码，只审计一次。
- **能真正清除内存**：`zeroize` 可以清除 Rust 持有的密钥。JVM 的字符串和对象做不到，这是 [安全设计 §2.3](04-security.md#23-密钥保管)「用完清除」的前提。
- **库成熟**：`rust-bitcoin`、`secp256k1`、`alloy-primitives`、`ed25519-dalek` 都是生产级依赖。
- **确定性签名**：libsecp256k1 默认用 RFC 6979，也提供不用辅助随机数的 Schnorr 签名，符合 [安全设计 §2.10](04-security.md#210-安装与升级)。

代价是多一门语言，构建链变长。网页端的 WASM 还要求 CSP 放开 `'wasm-unsafe-eval'`，见[安全设计 §3](04-security.md#3-网页端)。

### 2.2 冷钱包界面用 Compose Multiplatform

- 界面完全是自定义设计，不需要贴合系统外观，共用一套界面正合适。
- Android 的 Keystore、`FLAG_SECURE`、隐藏悬浮窗都是 Kotlin 直接调用，不用经过桥接。
- iOS 版已经稳定。目标是界面和流程逻辑共用，只为每个平台实现第 3 节的接口。
- 只用 Foundation 层，不引入 Material 组件库。冷钱包依赖越少，审计面越小，也避免和令牌体系打架。导航用一个简单的状态栈，不引入导航库。

### 2.3 网页 UI 库用 Base UI

网页端的 CSP 禁止内联脚本，也限制内联样式，这决定了选型：

| 候选                 | 结论                                                                                    |
| -------------------- | --------------------------------------------------------------------------------------- |
| Base UI + Tailwind   | **选用**。官方提供 CSP Provider，可以关闭组件内联的样式标签。无样式，完全由令牌控制外观 |
| Radix UI + shadcn/ui | 备选。生态最大，但没有查到等价的 CSP 控制                                               |
| Ant Design           | 不选。样式在运行时注入，与严格 CSP 冲突，体积大                                         |
| MUI                  | 不选。视觉语言难以覆盖成我们的设计                                                      |

组件层放在 `apps/web/src/ui/`，代码在自己仓库里，不依赖组件注册表，升级时逐个审查。

### 2.4 网关用 Rust

和核心库同一门语言，复用网络定义。单个二进制，自部署时同时托管网页端静态文件，满足同域部署。

### 2.5 不选的方案

| 方案                                  | 原因                                                                                                                  |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Flutter                               | 可以跨端，但引擎是预编译二进制，可复现构建更难。安全 API 都要走平台通道                                               |
| React Native                          | 冷钱包里放 JS 运行时和庞大的 npm 依赖树，审计面不可接受                                                               |
| Android 用 Compose，iOS 用 SwiftUI    | 约 20 个页面要写两遍，流程逻辑也要写两遍。核心库已经解决了安全逻辑，界面重写没有收益。如果 iOS 版体验不达标，这是退路 |
| 网页端用 TS 库实现交易逻辑            | 协议和解析要写两份，两端容易不一致                                                                                    |
| Gobley（UniFFI 的 Kotlin 多平台绑定） | 没查到 2026 年的发布，维护状态不明。用第 3 节的接口注入，不依赖它                                                     |

## 3. iOS 扩展路径

冷钱包的代码分三层：

| 层        | 内容                                 | iOS 要做什么                       |
| --------- | ------------------------------------ | ---------------------------------- |
| Rust 核心 | `bitone-core`、`bitone-signer`       | 不用改，编译到 iOS 目标            |
| 共享界面  | `apps/mobile/shared`，全部页面和流程 | 不用改                             |
| 平台接口  | 见下表                               | 用 Swift 或 Kotlin/Native 各写一份 |

| 接口          | Android                                                                     | iOS                                       |
| ------------- | --------------------------------------------------------------------------- | ----------------------------------------- |
| `WalletCore`  | UniFFI Kotlin                                                               | UniFFI Swift，启动时注入共享层            |
| `HardwareKey` | Keystore AES，StrongBox 优先                                                | Secure Enclave 的 P-256 密钥做 ECIES 包裹 |
| `SelfCheck`   | 系统 API                                                                    | 能力较弱，见下                            |
| `ScreenGuard` | `FLAG_SECURE`、隐藏悬浮窗                                                   | 能力较弱，见下                            |
| `QrScanner`   | CameraX + zxing-cpp。不用 Google Play 服务和 ML Kit，录像栈和 media3 已排除 | AVFoundation                              |
| `Brightness`  | 窗口亮度                                                                    | `UIScreen`                                |

**iOS 上做不到的防护**，要在安全设计里单独评估：

- 没有公开 API 读取飞行模式状态，也无法列举无障碍服务。
- 没有 `FLAG_SECURE` 的直接等价物，只能检测截屏事后告警。
- 无法做到「应用没有联网权限」，需要从设计上重新论证气隙。

所以 iOS 是否适合做冷钱包要单独评估，也可以只做观察钱包。这个架构两种都支持，核心库和界面层都能复用。

## 4. 工程结构

```
Cargo.toml              Rust 工作区
crates/
  bitone-core/          网络、路径、地址、二维码协议、解析、分类。不含密钥
  bitone-signer/        助记词、种子加密、派生、签名。只给冷钱包
  bitone-ffi/           UniFFI 绑定
  bitone-wasm/          wasm-bindgen 绑定
  bitone-gateway/       网关
apps/
  web/                  网页端
  mobile/
    shared/             Compose Multiplatform 共享界面
    androidApp/         Android 应用
design/
  tokens.json           设计令牌，唯一来源
  generate.mjs          生成 CSS 和 Kotlin
```

`bitone-signer` 不进 WASM，网页端的构建产物里不存在任何签名代码。

## 5. 依赖与构建

- **锁定版本**：`Cargo.lock`、`pnpm-lock.yaml` 和 Gradle 版本目录都进仓库。
- **审查**：Rust 用 `cargo audit` 和 `cargo deny`。新增依赖要写明理由。
- **WASM**：`secp256k1` 要编译 C 代码，Apple clang 没有 wasm 目标，本机用 Homebrew 的 LLVM，发布用容器。
- **工具链**：Homebrew 的 cargo 会绕过 `rust-toolchain.toml`，所以构建都走 Makefile。`make test` 跑全部检查。
- **Android 版本**：CMP 1.12 要求 compileSdk 至少 37。Gradle 9.8 超出 Kotlin 2.4 官方兼容表（到 9.7），实测通过，有问题就退回 9.7。
- **发布**：推送 `v1.2.3` 这样的 tag，版本号要和 `Cargo.toml` 一致。CI 全部通过后发布 GitHub Release，包含 Linux x86_64 和 arm64 的网关加网页端、未签名的 APK 和 `SHA256SUMS`。发布构建不用缓存。
- **APK 签名**：签名私钥不进 CI。Release 发布后在离线机器上签名，再用 `gh release upload` 上传。
- **代码裁剪**：发布版开 R8，只裁剪和优化，不混淆，堆栈和发布包审计都看得懂。JNA、UniFFI 和 zxing-cpp 靠反射或原生回调，规则在 `proguard-rules.pro`。
- **可复现构建**：发布版在固定的容器里构建。Kotlin 钱包的常见失败点是 dex 和 baseline profile 不一致，所以关闭 baseline profile，把构建版本写死。Rust 用 `--remap-path-prefix` 去掉本机路径。本机验证过：关掉 Gradle 构建缓存，换目录从干净状态构建两次，开启 R8 的未签名 APK 哈希一致。跨机器和容器内构建还没验证。
