#!/usr/bin/env bash
# 在真实的 java-tron 私链(tronbox/tre)上走通 TRX 转账和 TRC-20 转账.
# 交易由 bitone-core 构建,bitone-signer 核对和签名,tx::verify 核验后广播.用完删除容器.
# 用法:scripts/localnet-tron.sh
set -euo pipefail

IMAGE=${TRE_IMAGE:-tronbox/tre:latest}
NAME=bitone-tron-localnet
PORT=${LOCALNET_PORT:-19090}
NODE=http://127.0.0.1:$PORT
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PEER=TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE
# 最小的 TRC-20:构造函数给部署者 1000000000,只有 balanceOf 和 transfer.
# solc 0.8.20,evmVersion istanbul,optimizer runs 1.
ABI='[{"inputs":[],"stateMutability":"nonpayable","type":"constructor"},{"inputs":[{"internalType":"address","name":"","type":"address"}],"name":"balanceOf","outputs":[{"internalType":"uint256","name":"","type":"uint256"}],"stateMutability":"view","type":"function"},{"inputs":[{"internalType":"address","name":"to","type":"address"},{"internalType":"uint256","name":"value","type":"uint256"}],"name":"transfer","outputs":[{"internalType":"bool","name":"","type":"bool"}],"stateMutability":"nonpayable","type":"function"}]'
BYTECODE=608060405234801561001057600080fd5b50336000908152602081905260409020633b9aca0090556101e3806100366000396000f3fe608060405234801561001057600080fd5b50600436106100365760003560e01c806370a082311461003b578063a9059cbb1461006e575b600080fd5b61005b610049366004610125565b60006020819052908152604090205481565b6040519081526020015b60405180910390f35b61008161007c366004610147565b610091565b6040519015158152602001610065565b336000908152602081905260408120548211156100ad57600080fd5b33600090815260208190526040812080548492906100cc908490610187565b90915550506001600160a01b038316600090815260208190526040812080548492906100f990849061019a565b9091555060019150505b92915050565b80356001600160a01b038116811461012057600080fd5b919050565b60006020828403121561013757600080fd5b61014082610109565b9392505050565b6000806040838503121561015a57600080fd5b61016383610109565b946020939093013593505050565b634e487b7160e01b600052601160045260246000fd5b8181038181111561010357610103610171565b808201808211156101035761010361017156fea26469706673582212206ecdfbdd446b4213c5af453aff6af3a79585887ceb32580fcd5b519a6006c3a164736f6c63430008140033
export PATH="$(rustup which cargo | xargs dirname):$PATH"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

post() { curl -sS -X POST "$NODE$1" -H 'content-type: application/json' -d "$2"; }

# 从标准输入的 JSON 取值,取不到时打印空行:field txid
field() {
	python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if key.isdigit() else value.get(key, "") if isinstance(value, dict) else ""
print(value)' "$1"
}

fail() {
	echo "失败:$*" >&2
	exit 1
}

expect() {
	[ "$2" = "$3" ] || fail "$1:期望 $3,实际 $2"
	echo "通过:$1 = $2"
}

wallet() { "$WALLET" "$@"; }

broadcast() {
	local result
	result=$(post /wallet/broadcasthex "{\"transaction\":\"$1\"}")
	[ "$(printf '%s' "$result" | field result)" = True ] || {
		echo "$result"
		return 1
	}
}

wait_receipt() {
	local info number
	for _ in $(seq 1 60); do
		info=$(post /wallet/gettransactioninfobyid "{\"value\":\"$1\"}")
		number=$(printf '%s' "$info" | field blockNumber)
		if [ -n "$number" ]; then
			printf '%s' "$info"
			return 0
		fi
		sleep 1
	done
	fail "等待上链超时:$1"
}

# 引用最新区块,有效期 30 分钟.输出 ref_block_bytes ref_block_hash expiration timestamp.
block_params() {
	post /wallet/getnowblock '{}' | python3 -c '
import json, sys
block = json.load(sys.stdin)
block_id = block["blockID"]
timestamp = block["block_header"]["raw_data"]["timestamp"]
print(block_id[12:16], block_id[16:32], timestamp + 1800000, timestamp)'
}

balance() { post /wallet/getaccount "{\"address\":\"$1\",\"visible\":true}" | field balance; }

token_balance() {
	local hex
	hex=$(
		python3 - "$1" <<'PY'
import hashlib, sys
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = 0
for c in sys.argv[1]:
    n = n * 58 + A.index(c)
print(n.to_bytes(25, "big")[1:21].hex().rjust(64, "0"))
PY
	)
	post /wallet/triggerconstantcontract "{\"owner_address\":\"$OWNER\",\"contract_address\":\"$TOKEN\",\"function_selector\":\"balanceOf(address)\",\"parameter\":\"$hex\",\"visible\":true}" |
		python3 -c 'import json,sys; print(int(json.load(sys.stdin)["constant_result"][0], 16))'
}

cd "$ROOT"
cargo build -q -p bitone-signer --example localnet
WALLET=${CARGO_TARGET_DIR:-$ROOT/target}/debug/examples/localnet

cleanup
docker run -d --name "$NAME" -p "127.0.0.1:$PORT:9090" -e accounts=1 "$IMAGE" >/dev/null

for _ in $(seq 1 120); do
	ACCOUNTS=$(curl -s "$NODE/admin/accounts-json" 2>/dev/null || true)
	case "$ACCOUNTS" in *privateKeys*) break ;; esac
	sleep 1
done
FUNDED_KEY=$(printf '%s' "$ACCOUNTS" | field privateKeys.0)
FUNDED=$(docker logs "$NAME" 2>&1 | sed -n 's/^(0) \(T[A-Za-z0-9]*\) .*/\1/p' | head -1)
for _ in $(seq 1 120); do
	NUMBER=$(post /wallet/getnowblock '{}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["block_header"]["raw_data"].get("number", 0))')
	[ "$NUMBER" -ge 2 ] && break
	sleep 1
done
echo "节点版本:$(curl -s "$NODE/wallet/getnodeinfo" | field configNodeInfo.codeVersion)"

OWNER=$(wallet tron-address)
echo "付款账户 $OWNER"

# 准备资金.
UNSIGNED=$(post /wallet/createtransaction "{\"owner_address\":\"$FUNDED\",\"to_address\":\"$OWNER\",\"amount\":5000000000,\"visible\":true}")
FUNDING=$(wallet tron-sign-with-key "$FUNDED_KEY" "$(printf '%s' "$UNSIGNED" | field raw_data_hex)")
broadcast "$FUNDING" || fail "打款失败"
wait_receipt "$(printf '%s' "$UNSIGNED" | field txID)" >/dev/null
expect "打款到账" "$(balance "$OWNER")" 5000000000

read -r REF_BYTES REF_HASH EXPIRATION TIMESTAMP < <(block_params)
read -r RAW HASH < <(wallet tron-transfer "$PEER" 1500000 "$REF_BYTES" "$REF_HASH" "$EXPIRATION" "$TIMESTAMP")
broadcast "$RAW" || fail "TRX 转账被拒绝"
wait_receipt "$HASH" >/dev/null
expect "收款方余额" "$(balance "$PEER")" 1500000
ID=$(post /wallet/gettransactionbyid "{\"value\":\"$HASH\"}" | field txID)
expect "节点算出的 txID" "$ID" "$HASH"

# 同一笔不能再上链.
if OUTPUT=$(broadcast "$RAW"); then fail "重放成功了"; fi
echo "通过:重放被拒绝 $(printf '%s' "$OUTPUT" | field code)"

# 改掉签名里的一个字节,节点必须拒绝.
read -r RAW HASH < <(wallet tron-transfer "$PEER" 1500001 "$REF_BYTES" "$REF_HASH" "$EXPIRATION" "$TIMESTAMP")
BAD=$(printf '%s' "$RAW" | python3 -c '
import sys
raw = sys.stdin.read().strip()
offset = len(raw) - 40
print(raw[:offset] + ("00" if raw[offset:offset + 2] != "00" else "01") + raw[offset + 2:])')
if OUTPUT=$(broadcast "$BAD"); then fail "坏签名被接受了"; fi
echo "通过:坏签名被拒绝 $(printf '%s' "$OUTPUT" | field code)"

# 已过期的交易.构建时的时间窗口合法,但节点时钟已经过了 expiration.
PAST=$((TIMESTAMP - 3600000))
read -r RAW HASH < <(wallet tron-transfer "$PEER" 1500002 "$REF_BYTES" "$REF_HASH" "$((PAST + 1000))" "$PAST")
if OUTPUT=$(broadcast "$RAW"); then fail "过期交易被接受了"; fi
echo "通过:过期交易被拒绝 $(printf '%s' "$OUTPUT" | field code)"

# 部署合约.这笔由节点构建,冷钱包按"无法识别"核对后签名.
DEPLOY=$(post /wallet/deploycontract "$(
	python3 - "$OWNER" "$BYTECODE" "$ABI" <<'PY'
import json, sys
owner, bytecode, abi = sys.argv[1:]
print(json.dumps({
    "owner_address": owner, "visible": True, "name": "LocalToken", "bytecode": bytecode,
    "abi": abi, "fee_limit": 1000000000, "call_value": 0,
    "consume_user_resource_percent": 100, "origin_energy_limit": 10000000,
}))
PY
)")
read -r RAW HASH < <(wallet tron-sign "$(printf '%s' "$DEPLOY" | field raw_data_hex)")
expect "部署交易的 txID" "$HASH" "$(printf '%s' "$DEPLOY" | field txID)"
broadcast "$RAW" || fail "部署被拒绝"
RECEIPT=$(wait_receipt "$HASH")
expect "部署结果" "$(printf '%s' "$RECEIPT" | field receipt.result)" SUCCESS
TOKEN=$(
	python3 - "$(printf '%s' "$DEPLOY" | field contract_address)" <<'PY'
import hashlib, sys
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
payload = bytes.fromhex(sys.argv[1])
check = hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
n = int.from_bytes(payload + check, "big")
out = ""
while n:
    n, r = divmod(n, 58)
    out = A[r] + out
print(out)
PY
)
echo "合约 $TOKEN"
expect "部署者代币余额" "$(token_balance "$OWNER")" 1000000000

read -r REF_BYTES REF_HASH EXPIRATION TIMESTAMP < <(block_params)
read -r RAW HASH < <(wallet tron-token "$PEER" 1250000 "$TOKEN" 100000000 "$REF_BYTES" "$REF_HASH" "$EXPIRATION" "$TIMESTAMP")
broadcast "$RAW" || fail "TRC-20 转账被拒绝"
RECEIPT=$(wait_receipt "$HASH")
expect "TRC-20 转账结果" "$(printf '%s' "$RECEIPT" | field receipt.result)" SUCCESS
expect "收款方代币余额" "$(token_balance "$PEER")" 1250000
expect "付款方代币余额" "$(token_balance "$OWNER")" 998750000

echo "全部通过"
