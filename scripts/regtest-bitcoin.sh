#!/usr/bin/env bash
# 在 Docker 的 regtest 节点上验证 Bitcoin 的构建,核对,签名,核验.
# 用法:scripts/regtest-bitcoin.sh
# 每次都用新容器,退出时删除,可以重复运行.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$(rustup which cargo | xargs dirname):$PATH"

IMAGE=bitcoin/bitcoin:latest
export REGTEST_CONTAINER="bitone-regtest-$$"
trap 'docker rm -f "$REGTEST_CONTAINER" >/dev/null 2>&1 || true' EXIT

docker run -d --name "$REGTEST_CONTAINER" "$IMAGE" \
  bitcoind -regtest -server -rpcuser=bitone -rpcpassword=bitone -fallbackfee=0.0001 -txindex=1 >/dev/null

for _ in $(seq 1 60); do
  if docker exec "$REGTEST_CONTAINER" bitcoin-cli -regtest -rpcuser=bitone -rpcpassword=bitone getblockchaininfo >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

cargo run -q -p bitone-signer --example regtest_bitcoin
