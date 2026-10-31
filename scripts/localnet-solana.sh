#!/usr/bin/env bash
# 在真实的 solana-test-validator 上走通创建 nonce 账户、SOL 转账、SPL 代币转账。
# 交易由 bitone-core 构建，bitone-signer 核对和签名，tx::verify 核验后广播。用完删除容器。
#
# agave 的镜像只有 amd64。Docker Desktop 的转译层不支持 io_uring，v3 起的验证者启动时会断言失败，
# 所以固定用 v2.3.13。可以用 AGAVE_IMAGE 覆盖。
# 用法：scripts/localnet-solana.sh
set -euo pipefail

IMAGE=${AGAVE_IMAGE:-anzaxyz/agave:v2.3.13}
NAME=bitone-solana-localnet
PORT=${LOCALNET_PORT:-18899}
RPC=http://127.0.0.1:$PORT
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PEER=7S3P4HxJpyyigGzodYwHtCxZyUQe9JiBMHyRWXArAaKv
export PATH="$(rustup which cargo | xargs dirname):$PATH"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

rpc() {
  curl -sS -X POST -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" "$RPC"
}

# 从标准输入的 JSON 取值：field result.value.blockhash
field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if key.isdigit() else value[key]
print(value)' "$1"
}

fail() {
  echo "失败：$*" >&2
  exit 1
}

wallet() { "$WALLET" "$@"; }

send() {
  local encoded result
  encoded=$(printf '%s' "$1" | xxd -r -p | base64 | tr -d '\n')
  result=$(rpc sendTransaction "[\"$encoded\",{\"encoding\":\"base64\",\"preflightCommitment\":\"confirmed\"}]")
  case "$result" in
    *'"error"'*) echo "$result"; return 1 ;;
  esac
  printf '%s' "$result" | field result
}

confirm() {
  local status
  for _ in $(seq 1 60); do
    status=$(rpc getSignatureStatuses "[[\"$1\"]]")
    case "$status" in
      *'"err":null'*'"confirmationStatus":"confirmed"'* | *'"confirmationStatus":"confirmed"'*'"err":null'* | \
      *'"err":null'*'"confirmationStatus":"finalized"'* | *'"confirmationStatus":"finalized"'*'"err":null'*)
        return 0 ;;
      *'"err":{'*) fail "交易失败：$status" ;;
    esac
    sleep 0.5
  done
  fail "等待确认超时：$1"
}

nonce_value() {
  rpc getAccountInfo "[\"$NONCE\",{\"encoding\":\"jsonParsed\",\"commitment\":\"confirmed\"}]" |
    field result.value.data.parsed.info.blockhash
}

expect() {
  [ "$2" = "$3" ] || fail "$1：期望 $3，实际 $2"
  echo "通过：$1 = $2"
}

cd "$ROOT"
cargo build -q -p bitone-signer --example localnet
WALLET=${CARGO_TARGET_DIR:-$ROOT/target}/debug/examples/localnet

cleanup
docker run -d --name "$NAME" --platform linux/amd64 -p "127.0.0.1:$PORT:8899" --entrypoint sh "$IMAGE" \
  -c 'IP=$(hostname -i); exec solana-test-validator --ledger /tmp/ledger --bind-address $IP --gossip-host $IP --rpc-port 8899 --quiet' >/dev/null

# getHealth 在第 0 个 slot 就返回 ok，那时发的交易会丢，要等出块。
for _ in $(seq 1 180); do
  SLOT=$(rpc getSlot '[{"commitment":"confirmed"}]' 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("result", 0))' 2>/dev/null || echo 0)
  [ "$SLOT" -ge 40 ] && break
  sleep 1
done
[ "$SLOT" -ge 40 ] || fail "节点没有出块"
echo "节点版本：$(rpc getVersion '[]' | field result.solana-core)"

PAYER=$(wallet solana-address)
NONCE=$(python3 - "$PAYER" <<'PY'
import hashlib, sys
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = 0
for c in sys.argv[1]:
    n = n * 58 + A.index(c)
digest = hashlib.sha256(n.to_bytes(32, "big") + b"bitone-nonce" + bytes(32)).digest()
n = int.from_bytes(digest, "big")
out = ""
while n:
    n, r = divmod(n, 58)
    out = A[r] + out
print("1" * (len(digest) - len(digest.lstrip(b"\0"))) + out)
PY
)
echo "付款账户 $PAYER"
echo "nonce 账户 $NONCE"

for _ in $(seq 1 30); do
  rpc requestAirdrop "[\"$PAYER\",10000000000]" >/dev/null
  sleep 2
  BALANCE=$(rpc getBalance "[\"$PAYER\",{\"commitment\":\"confirmed\"}]" | field result.value)
  [ "$BALANCE" != 0 ] && break
done
expect "空投到账" "$BALANCE" 10000000000

# nonce 地址是公开的，任何人都能先往里存钱。预存款不能挡住创建。
rpc requestAirdrop "[\"$NONCE\",1000000]" >/dev/null
for _ in $(seq 1 30); do
  sleep 1
  FUNDED=$(rpc getBalance "[\"$NONCE\",{\"commitment\":\"confirmed\"}]" | field result.value)
  [ "$FUNDED" != 0 ] && break
done
expect "nonce 地址已被预存款" "$FUNDED" 1000000

# 创建 nonce 账户。
RENT=$(rpc getMinimumBalanceForRentExemption '[80]' | field result)
BLOCKHASH=$(rpc getLatestBlockhash '[{"commitment":"confirmed"}]' | field result.value.blockhash)
read -r RAW HASH < <(wallet solana-create-nonce "$BLOCKHASH" "$RENT")
SIGNATURE=$(send "$RAW")
expect "创建 nonce 的签名" "$SIGNATURE" "$HASH"
confirm "$SIGNATURE"
AUTHORITY=$(rpc getAccountInfo "[\"$NONCE\",{\"encoding\":\"jsonParsed\",\"commitment\":\"confirmed\"}]" |
  field result.value.data.parsed.info.authority)
expect "nonce 账户的 authority" "$AUTHORITY" "$PAYER"

# SOL 转账。
VALUE=$(nonce_value)
read -r RAW HASH < <(wallet solana-transfer "$PEER" 1500000000 "$VALUE")
SIGNATURE=$(send "$RAW")
expect "SOL 转账的签名" "$SIGNATURE" "$HASH"
confirm "$SIGNATURE"
BALANCE=$(rpc getBalance "[\"$PEER\",{\"commitment\":\"confirmed\"}]" | field result.value)
expect "收款方余额" "$BALANCE" 1500000000
NEXT=$(nonce_value)
[ "$NEXT" != "$VALUE" ] || fail "nonce 值没有推进"
echo "通过：nonce 值已推进 $VALUE -> $NEXT"

# 旧 nonce 值已经作废，同一笔和新构建的都不能再上链。
if OUTPUT=$(send "$RAW"); then fail "重放成功了：$OUTPUT"; fi
echo "通过：重放被拒绝 $(printf '%s' "$OUTPUT" | field error.message)"
read -r STALE _ < <(wallet solana-transfer "$PEER" 1 "$VALUE")
if OUTPUT=$(send "$STALE"); then fail "旧 nonce 值还能用：$OUTPUT"; fi
echo "通过：旧 nonce 值被拒绝 $(printf '%s' "$OUTPUT" | field error.message)"

# 准备代币：另一个账户建 mint，给付款账户铸币。
docker exec "$NAME" sh -c 'mkdir -p /root/.config/solana && solana-keygen new --no-bip39-passphrase --silent --force -o /root/.config/solana/id.json >/dev/null'
in_node() { docker exec "$NAME" sh -c "URL=http://\$(hostname -i):8899; $1"; }
in_node 'solana -u $URL airdrop 10 >/dev/null'
MINT=$(in_node 'spl-token -u $URL create-token --decimals 6 --output json' | field commandOutput.address)
in_node "spl-token -u \$URL create-account $MINT --owner $PAYER --fee-payer /root/.config/solana/id.json >/dev/null"
token_account() {
  rpc getTokenAccountsByOwner "[\"$1\",{\"mint\":\"$MINT\"},{\"encoding\":\"jsonParsed\",\"commitment\":\"confirmed\"}]" |
    field result.value.0.pubkey
}
SOURCE=$(token_account "$PAYER")
in_node "spl-token -u \$URL mint $MINT 1000 $SOURCE >/dev/null"
echo "mint ${MINT}"
echo "付款账户的代币账户 ${SOURCE}"

# 代币转账：先建收款方账户。
VALUE=$(nonce_value)
read -r RAW HASH < <(wallet solana-token "$PEER" 1250000 "$MINT" 6 "$VALUE" 1)
SIGNATURE=$(send "$RAW")
expect "代币转账的签名" "$SIGNATURE" "$HASH"
confirm "$SIGNATURE"
PEER_TOKEN=$(token_account "$PEER")
AMOUNT=$(rpc getTokenAccountBalance "[\"$PEER_TOKEN\",{\"commitment\":\"confirmed\"}]" | field result.value.amount)
expect "收款方代币余额" "$AMOUNT" 1250000

# 代币转账：账户已经存在。
VALUE=$(nonce_value)
read -r RAW HASH < <(wallet solana-token "$PEER" 750000 "$MINT" 6 "$VALUE" 0)
SIGNATURE=$(send "$RAW")
confirm "$SIGNATURE"
AMOUNT=$(rpc getTokenAccountBalance "[\"$PEER_TOKEN\",{\"commitment\":\"confirmed\"}]" | field result.value.amount)
expect "收款方代币余额" "$AMOUNT" 2000000
AMOUNT=$(rpc getTokenAccountBalance "[\"$SOURCE\",{\"commitment\":\"confirmed\"}]" | field result.value.amount)
expect "付款方代币余额" "$AMOUNT" 998000000

echo "全部通过"
