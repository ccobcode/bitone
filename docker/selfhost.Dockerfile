# 自部署:网页端和网关同域,一个容器.同域时 CSP 的 connect-src 不用放宽.
# 构建:docker build -f docker/selfhost.Dockerfile -t bitone .
# 运行:docker run -p 127.0.0.1:8787:8787 bitone
# 镜像自带示例配置.要填 API key 时挂载自己的配置:-v ./gateway.toml:/etc/bitone/gateway.toml

FROM rust:1.95-slim-bookworm@sha256:d7482085ff5b415f84dba5647ae71606650bdef00db7aeb69f4b3d170c3e4082 AS rust
RUN apt-get update && apt-get install -y --no-install-recommends clang llvm pkg-config
RUN rustup target add wasm32-unknown-unknown
RUN cargo install wasm-bindgen-cli --version 0.2.130 --locked
WORKDIR /src
# 不复制 rust-toolchain.toml,否则会多下载 Android 和 iOS 目标.
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates crates
# 内置的 ERC-7730 描述文件在这里,编译时读取.
COPY schemas schemas
# 容器里的 clang 自带 wasm 目标.
ENV CC_wasm32_unknown_unknown=clang AR_wasm32_unknown_unknown=llvm-ar
RUN cargo build --release -p bitone-gateway \
    && cargo build --release -p bitone-wasm --target wasm32-unknown-unknown \
    && wasm-bindgen target/wasm32-unknown-unknown/release/bitone_wasm.wasm --target web --out-dir /wasm

FROM node:22-alpine@sha256:fe4f6d74e20370acb04ad889077d14d0f64a9c8ea6a42bc60cddf1e844699914 AS web
RUN corepack enable
WORKDIR /src
COPY design design
COPY apps/web apps/web
COPY --from=rust /wasm apps/web/src/core/pkg
WORKDIR /src/apps/web
RUN pnpm install --frozen-lockfile && pnpm build

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=rust /src/target/release/bitone-gateway /usr/local/bin/bitone-gateway
COPY --from=rust /src/crates/bitone-gateway/gateway.example.toml /etc/bitone/gateway.toml
COPY --from=web /src/apps/web/dist /srv/bitone/web
COPY LICENSE /usr/share/doc/bitone/LICENSE
USER 65534
EXPOSE 8787
ENTRYPOINT ["bitone-gateway", "--config", "/etc/bitone/gateway.toml", "--listen", "0.0.0.0:8787", "--web-dir", "/srv/bitone/web"]
