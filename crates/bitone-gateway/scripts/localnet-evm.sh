#!/usr/bin/env bash
# 用真实的 anvil 节点跑一遍网关的 EVM 接口：nonce、fees、estimate、broadcast、tx、回滚交易报 failed。
# 全程只碰本机。结束时停止并删除容器，退出码非 0 表示有检查没通过。
#
# 用法：crates/bitone-gateway/scripts/localnet-evm.sh
# 依赖：Docker、rustup、python3、curl。
set -euo pipefail

IMAGE=ghcr.io/foundry-rs/foundry:latest
CONTAINER="bitone-gateway-anvil-$$"
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=$(mktemp -d)
GATEWAY_PID=""
PASSED=0
FAILED=0

# anvil 的默认开发账户，公开的测试私钥，只在本机这条链上有意义。
KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
FROM=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
TO=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
# 没有余额的账户。
POOR_KEY=0x0000000000000000000000000000000000000000000000000000000000000abc
# 运行时代码是 PUSH1 0 PUSH1 0 REVERT，任何调用都回滚。
REVERTER_INIT=0x6460006000fd6000526005601bf3

cleanup() {
  [[ -n "$GATEWAY_PID" ]] && kill "$GATEWAY_PID" 2>/dev/null || true
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

check() {
  local name=$1 actual=$2 expected=$3
  if [[ "$actual" == "$expected" ]]; then
    echo "通过 $name"
    PASSED=$((PASSED + 1))
  else
    echo "失败 ${name}：期望 ${expected}，实际 ${actual}"
    FAILED=$((FAILED + 1))
  fi
}

# 从标准输入的 JSON 里取一个字段，路径用点分隔。
field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value.get(key)
print("null" if value is None else value)' "$1"
}

api_get() { curl -sS -m 20 "$BASE$1"; }
api_post() { curl -sS -m 20 -H 'content-type: application/json' -d "$2" "$BASE$1"; }
rpc() { curl -sS -m 20 -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" "$NODE"; }
mine() { rpc anvil_mine "[\"$(printf '0x%x' "$1")\"]" >/dev/null; }
cast_in_container() { docker exec "$CONTAINER" cast "$@"; }
tx_field() { api_post /v1/ethereum/tx "{\"hashes\":[\"$1\"]}" | field "txs.0.$2"; }
broadcast() { api_post /v1/ethereum/broadcast "{\"raw\":\"$1\"}"; }

# 签一笔交易。手续费写死，这样不依赖估算结果。
sign() {
  cast_in_container mktx --rpc-url http://127.0.0.1:8545 --private-key "$1" --gas-price 100gwei --priority-gas-price 1gwei "${@:2}"
}

echo "== 启动 anvil"
# 每个 epoch 2 个 slot：safe 比 latest 落后 2 块，finalized 落后 4 块，几次出块就能走完全部阶段。
# 关闭自动出块，才能看到内存池状态。
docker run -d --name "$CONTAINER" -p 127.0.0.1::8545 --entrypoint anvil "$IMAGE" \
  --host 0.0.0.0 --no-mining --slots-in-an-epoch 2 >/dev/null
NODE="http://$(docker port "$CONTAINER" 8545/tcp | head -1)"
for _ in $(seq 1 50); do
  rpc eth_chainId '[]' >/dev/null 2>&1 && break
  sleep 0.2
done
docker exec "$CONTAINER" anvil --version | head -1
mine 1

echo "== 启动网关"
PATH="$(rustup which cargo | xargs dirname):$PATH" cargo build -q -p bitone-gateway --manifest-path "$ROOT/Cargo.toml"
cat > "$WORK/gateway.toml" <<CONFIG
listen = "127.0.0.1:0"
[networks.ethereum]
upstreams = ["$NODE"]
CONFIG
"$ROOT/target/debug/bitone-gateway" "$WORK/gateway.toml" 2> "$WORK/gateway.log" &
GATEWAY_PID=$!
for _ in $(seq 1 50); do
  grep -q 监听 "$WORK/gateway.log" && break
  sleep 0.1
done
BASE="http://$(sed -n 's/.*监听 //p' "$WORK/gateway.log" | head -1)"

echo "== 只读接口"
check "version 列出网络" "$(api_get /v1/version | field networks.0)" ethereum
REPLY=$(api_post /v1/ethereum/nonce "{\"addresses\":[\"$FROM\"]}")
check "nonce latest" "$(echo "$REPLY" | field "nonces.$FROM.latest")" 0
check "nonce pending" "$(echo "$REPLY" | field "nonces.$FROM.pending")" 0

FEES=$(api_get /v1/ethereum/fees)
echo "手续费 $FEES"
check "fees 类型" "$(echo "$FEES" | field kind)" evm
check "fees 最高单价不低于基础费" "$(echo "$FEES" | python3 -c '
import json, sys
fees = json.load(sys.stdin)
base = int(fees["baseFee"])
print(all(int(fees[tier]["maxFee"]) >= base and int(fees[tier]["priority"]) >= 0 for tier in ("slow", "medium", "fast")))')" True

REPLY=$(api_post /v1/ethereum/estimate "{\"from\":\"$FROM\",\"to\":\"$TO\",\"value\":\"1000\"}")
check "estimate 普通转账" "$(echo "$REPLY" | field gas)" 21000

echo "== 广播和状态"
RAW=$(sign "$KEY" --nonce 0 --gas-limit 21000 --value 1000 "$TO")
HASH=$(cast_in_container keccak "$RAW")
check "广播前查不到" "$(tx_field "$HASH" status)" unknown

REPLY=$(broadcast "$RAW")
check "broadcast 接收" "$(echo "$REPLY" | field status)" accepted
check "broadcast 哈希和本地计算一致" "$(echo "$REPLY" | field hash)" "$HASH"
check "上链前是 pending" "$(tx_field "$HASH" status)" pending
REPLY=$(api_post /v1/ethereum/nonce "{\"addresses\":[\"$FROM\"]}")
check "nonce pending 增加" "$(echo "$REPLY" | field "nonces.$FROM.pending")" 1
check "nonce latest 不变" "$(echo "$REPLY" | field "nonces.$FROM.latest")" 0

REPLY=$(broadcast "$RAW")
echo "重复广播 $REPLY"
check "内存池里重复广播仍是成功" "$(echo "$REPLY" | field status)" accepted

mine 1
check "打包后 included" "$(tx_field "$HASH" status)" included
check "打包后阶段 latest" "$(tx_field "$HASH" stage)" latest
check "手续费大于 0" "$(tx_field "$HASH" fee | python3 -c 'print(int(input()) > 0)')" True

REPLY=$(broadcast "$RAW")
echo "上链后重复广播 $REPLY"
check "上链后重复广播仍是成功" "$(echo "$REPLY" | field status)" accepted

mine 2
check "落后 2 块进入 safe" "$(tx_field "$HASH" stage)" safe
check "safe 还不是完成" "$(tx_field "$HASH" status)" included
mine 2
check "落后 4 块进入 finalized" "$(tx_field "$HASH" stage)" finalized
check "finalized 才是完成" "$(tx_field "$HASH" status)" finalized

echo "== 节点拒绝"
OTHER=$(sign "$KEY" --nonce 0 --gas-limit 21000 --value 2000 "$TO")
REPLY=$(broadcast "$OTHER")
echo "同 nonce 的另一笔 $REPLY"
check "nonce 被别的交易用掉要报拒绝" "$(echo "$REPLY" | field error.code)" broadcast_rejected

POOR=$(sign "$POOR_KEY" --nonce 0 --gas-limit 21000 --value 1000 "$TO")
REPLY=$(broadcast "$POOR")
echo "余额不足 $REPLY"
check "余额不足要报拒绝" "$(echo "$REPLY" | field error.code)" broadcast_rejected

echo "== 回滚的交易"
RAW=$(sign "$KEY" --nonce 1 --gas-limit 200000 --create "$REVERTER_INIT")
DEPLOY=$(cast_in_container keccak "$RAW")
check "部署合约" "$(broadcast "$RAW" | field status)" accepted
mine 1
CONTRACT=$(rpc eth_getTransactionReceipt "[\"$DEPLOY\"]" | field result.contractAddress)
check "部署成功" "$(tx_field "$DEPLOY" status)" included

REPLY=$(api_post /v1/ethereum/estimate "{\"from\":\"$FROM\",\"to\":\"$CONTRACT\"}")
echo "估算回滚 $REPLY"
check "估算回滚是 invalid_request，不是上游故障" "$(echo "$REPLY" | field error.code)" invalid_request

RAW=$(sign "$KEY" --nonce 2 --gas-limit 100000 "$CONTRACT")
REVERTED=$(cast_in_container keccak "$RAW")
check "回滚交易也能广播" "$(broadcast "$RAW" | field status)" accepted
mine 1
check "回滚交易上链后是 failed" "$(tx_field "$REVERTED" status)" failed
check "failed 带阶段" "$(tx_field "$REVERTED" stage)" latest
check "failed 手续费已扣除" "$(tx_field "$REVERTED" fee | python3 -c 'print(int(input()) > 0)')" True
mine 4
check "最终确定后仍然是 failed" "$(tx_field "$REVERTED" status)" failed
check "最终确定后阶段是 finalized" "$(tx_field "$REVERTED" stage)" finalized

echo
echo "通过 $PASSED 项，失败 $FAILED 项"
[[ "$FAILED" -eq 0 ]]
