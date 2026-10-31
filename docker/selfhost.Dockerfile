# 自部署：网页端和网关同域，一个容器。见 docs/04-security.md 第 3 节。
# 构建：docker build -f docker/selfhost.Dockerfile -t bitone .
# 运行：docker run -p 8787:8787 -v ./gateway.toml:/etc/bitone/gateway.toml bitone

FROM rust:1.95-slim-bookworm AS rust
RUN apt-get update && apt-get install -y --no-install-recommends clang llvm pkg-config
RUN rustup target add wasm32-unknown-unknown
RUN cargo install wasm-bindgen-cli --version 0.2.130 --locked
WORKDIR /src
# 不复制 rust-toolchain.toml，否则会多下载 Android 和 iOS 目标。
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates crates
# 容器里的 clang 自带 wasm 目标。
ENV CC_wasm32_unknown_unknown=clang AR_wasm32_unknown_unknown=llvm-ar
RUN cargo build --release -p bitone-gateway \
    && cargo build --release -p bitone-wasm --target wasm32-unknown-unknown \
    && wasm-bindgen target/wasm32-unknown-unknown/release/bitone_wasm.wasm --target web --out-dir /wasm

FROM node:22-alpine AS web
RUN corepack enable
WORKDIR /src
COPY design design
COPY apps/web apps/web
COPY --from=rust /wasm apps/web/src/core/pkg
WORKDIR /src/apps/web
RUN pnpm install --frozen-lockfile && pnpm build

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=rust /src/target/release/bitone-gateway /usr/local/bin/bitone-gateway
COPY --from=web /src/apps/web/dist /srv/bitone/web
USER 65534
EXPOSE 8787
ENTRYPOINT ["bitone-gateway", "--config", "/etc/bitone/gateway.toml"]
