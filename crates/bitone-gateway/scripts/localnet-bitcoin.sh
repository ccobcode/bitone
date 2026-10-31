#!/usr/bin/env bash
# 用真实的 bitcoind regtest 加 electrs（Esplora 接口）跑一遍网关的 Bitcoin 接口。
# 全程只碰本机。结束时停止并删除容器和网络，退出码非 0 表示有检查没通过。
#
# 网关的地址校验不接受 bcrt1 地址，而 legacy 地址和测试网共用版本字节，
# 所以这里把网关配成 btc-testnet4，查询都用 legacy 地址。
#
# 用法：crates/bitone-gateway/scripts/localnet-bitcoin.sh
# 依赖：Docker、rustup、python3、curl。
set -euo pipefail

BITCOIND_IMAGE=bitcoin/bitcoin:latest
ELECTRS_IMAGE=ghcr.io/vulpemventures/electrs:latest
SUFFIX=$$
NET="bitone-gateway-btc-$SUFFIX"
NODE="bitone-gateway-bitcoind-$SUFFIX"
INDEXER="bitone-gateway-electrs-$SUFFIX"
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=$(mktemp -d)
GATEWAY_PID=""
PASSED=0
FAILED=0

cleanup() {
  [[ -n "$GATEWAY_PID" ]] && kill "$GATEWAY_PID" 2>/dev/null || true
  docker rm -f "$NODE" "$INDEXER" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
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

field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value.get(key)
print("null" if value is None else value)' "$1"
}

cli() { docker exec "$NODE" bitcoin-cli -regtest -rpcuser=a -rpcpassword=b -rpcwallet=w "$@"; }
api_get() { curl -sS -m 20 "$BASE$1"; }
api_post() { curl -sS -m 20 -H 'content-type: application/json' -d "$2" "$BASE$1"; }
tx_field() { api_post /v1/$NETWORK/tx "{\"hashes\":[\"$1\"]}" | field "txs.0.$2"; }
broadcast() { api_post /v1/$NETWORK/broadcast "{\"raw\":\"$1\"}"; }
height() { curl -sS -m 5 "$ESPLORA/blocks/tip/height"; }

# 出块并等 electrs 索引到新高度。
mine() {
  cli generatetoaddress "$1" "$MINER" >/dev/null
  local target
  target=$(cli getblockcount)
  for _ in $(seq 1 100); do
    [[ "$(height 2>/dev/null)" == "$target" ]] && return
    sleep 0.2
  done
  echo "electrs 没有跟上出块" >&2
  exit 1
}

# 等 electrs 看到内存池里的交易。
wait_seen() {
  for _ in $(seq 1 100); do
    [[ "$(curl -s -o /dev/null -w '%{http_code}' "$ESPLORA/tx/$1")" == 200 ]] && return
    sleep 0.2
  done
  echo "electrs 没有看到交易" >&2
  exit 1
}

# 签一笔转账但不广播。
signed_payment() {
  local unsigned funded
  unsigned=$(cli createrawtransaction '[]' "{\"$1\":$2}")
  funded=$(cli fundrawtransaction "$unsigned" | field hex)
  cli signrawtransactionwithwallet "$funded" | field hex
}

NETWORK=btc-testnet4

echo "== 启动 bitcoind 和 electrs"
docker network create "$NET" >/dev/null
docker run -d --name "$NODE" --network "$NET" --entrypoint bitcoind "$BITCOIND_IMAGE" \
  -regtest -server -txindex -fallbackfee=0.0002 -rpcuser=a -rpcpassword=b \
  -rpcbind=0.0.0.0 -rpcallowip=0.0.0.0/0 >/dev/null
for _ in $(seq 1 100); do
  docker exec "$NODE" bitcoin-cli -regtest -rpcuser=a -rpcpassword=b getblockchaininfo >/dev/null 2>&1 && break
  sleep 0.2
done
docker exec "$NODE" bitcoind --version | head -1
docker exec "$NODE" bitcoin-cli -regtest -rpcuser=a -rpcpassword=b createwallet w >/dev/null
MINER=$(cli getnewaddress "" legacy)
cli generatetoaddress 101 "$MINER" >/dev/null

docker run -d --name "$INDEXER" --network "$NET" -p 127.0.0.1::3002 "$ELECTRS_IMAGE" \
  -vv --network regtest --daemon-dir /tmp --daemon-rpc-addr "$NODE:18443" --cookie a:b \
  --http-addr 0.0.0.0:3002 --electrum-rpc-addr 0.0.0.0:50000 --jsonrpc-import >/dev/null
ESPLORA="http://$(docker port "$INDEXER" 3002/tcp | head -1)"
for _ in $(seq 1 150); do
  [[ "$(height 2>/dev/null)" == "101" ]] && break
  sleep 0.2
done
check "electrs 索引完成" "$(height)" 101

echo "== 启动网关"
PATH="$(rustup which cargo | xargs dirname):$PATH" cargo build -q -p bitone-gateway --manifest-path "$ROOT/Cargo.toml"
cat > "$WORK/gateway.toml" <<CONFIG
listen = "127.0.0.1:0"
[networks.$NETWORK]
upstreams = ["$ESPLORA"]
CONFIG
"$ROOT/target/debug/bitone-gateway" "$WORK/gateway.toml" 2> "$WORK/gateway.log" &
GATEWAY_PID=$!
for _ in $(seq 1 50); do
  grep -q 监听 "$WORK/gateway.log" && break
  sleep 0.1
done
BASE="http://$(sed -n 's/.*监听 //p' "$WORK/gateway.log" | head -1)"

echo "== 只读接口"
FEES=$(api_get /v1/$NETWORK/fees)
echo "手续费 $FEES"
check "fees 类型" "$(echo "$FEES" | field kind)" bitcoin
PAYEE=$(cli getnewaddress "" legacy)
REPLY=$(api_post /v1/$NETWORK/balances "{\"addresses\":[\"$PAYEE\"]}")
check "没有记录的地址余额为 0" "$(echo "$REPLY" | field balances.0.amount)" 0
REPLY=$(api_post /v1/$NETWORK/utxos "{\"addresses\":[\"$PAYEE\"]}")
check "没有 UTXO" "$(echo "$REPLY" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["utxos"]))')" 0

echo "== 广播和确认数"
RAW=$(signed_payment "$PAYEE" 0.5)
TXID=$(cli decoderawtransaction "$RAW" | field txid)
check "广播前查不到" "$(tx_field "$TXID" status)" unknown

REPLY=$(broadcast "$RAW")
check "broadcast 接收" "$(echo "$REPLY" | field status)" accepted
check "broadcast 的 txid 和节点一致" "$(echo "$REPLY" | field hash)" "$TXID"
wait_seen "$TXID"
check "内存池里是 pending" "$(tx_field "$TXID" status)" pending
check "pending 的确认数是 0" "$(tx_field "$TXID" confirmations)" 0

REPLY=$(broadcast "$RAW")
echo "重复广播 $REPLY"
check "内存池里重复广播仍是成功" "$(echo "$REPLY" | field status)" accepted

REPLY=$(api_post /v1/$NETWORK/utxos "{\"addresses\":[\"$PAYEE\"]}")
check "内存池 UTXO 高度是 0" "$(echo "$REPLY" | field utxos.0.height)" 0
REPLY=$(api_post /v1/$NETWORK/balances "{\"addresses\":[\"$PAYEE\"]}")
check "余额不含内存池" "$(echo "$REPLY" | field balances.0.amount)" 0

mine 1
check "打包后 included" "$(tx_field "$TXID" status)" included
check "打包后 1 个确认" "$(tx_field "$TXID" confirmations)" 1
check "手续费大于 0" "$(tx_field "$TXID" fee | python3 -c 'print(int(input()) > 0)')" True
check "Bitcoin 没有阶段" "$(tx_field "$TXID" stage)" null
REPLY=$(api_post /v1/$NETWORK/utxos "{\"addresses\":[\"$PAYEE\"]}")
check "UTXO 金额" "$(echo "$REPLY" | field utxos.0.amount)" 50000000
check "UTXO 高度" "$(echo "$REPLY" | field utxos.0.height)" 102

REPLY=$(broadcast "$RAW")
echo "上链后重复广播 $REPLY"
check "上链后重复广播仍是成功" "$(echo "$REPLY" | field status)" accepted

mine 4
check "5 个确认还是 included" "$(tx_field "$TXID" status)" included
mine 1
check "6 个确认是 finalized" "$(tx_field "$TXID" status)" finalized
check "确认数是 6" "$(tx_field "$TXID" confirmations)" 6

echo "== raw-txs"
REPLY=$(api_post /v1/$NETWORK/raw-txs "{\"txids\":[\"$TXID\",\"$(printf '0%.0s' {1..64})\"]}")
check "raw-txs 返回节点的原始交易" "$(echo "$REPLY" | field "txs.$TXID")" "$(cli getrawtransaction "$TXID")"
check "查不到的 txid 不出现" "$(echo "$REPLY" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["txs"]))')" 1

echo "== 节点拒绝"
SPENT=$(cli decoderawtransaction "$RAW" | python3 -c '
import json, sys
inputs = json.load(sys.stdin)["vin"]
print(json.dumps([{"txid": i["txid"], "vout": i["vout"]} for i in inputs]))')
OTHER=$(cli getnewaddress "" legacy)
UNSIGNED=$(cli createrawtransaction "$SPENT" "{\"$OTHER\":1}")
CONFLICT=$(cli signrawtransactionwithwallet "$UNSIGNED" | field hex)
REPLY=$(broadcast "$CONFLICT")
echo "花已花费的输入 $REPLY"
check "双花要报拒绝" "$(echo "$REPLY" | field error.code)" broadcast_rejected

echo "== 交易花掉后再重复广播"
# 把收款的 UTXO 也花掉，这时节点不再说交易已知，只说输入不存在。
SPEND=$(cli decoderawtransaction "$RAW" | python3 -c "
import json, sys
tx = json.load(sys.stdin)
vout = [o['n'] for o in tx['vout'] if o['scriptPubKey'].get('address') == '$PAYEE'][0]
print(json.dumps([{'txid': tx['txid'], 'vout': vout}]))")
UNSIGNED=$(cli createrawtransaction "$SPEND" "{\"$OTHER\":0.4999}")
SECOND=$(cli signrawtransactionwithwallet "$UNSIGNED" | field hex)
check "花掉收款" "$(broadcast "$SECOND" | field status)" accepted
mine 1
REPLY=$(broadcast "$RAW")
echo "输入已花费后重复广播 $REPLY"
check "节点只说输入不存在时，链上查到就是成功" "$(echo "$REPLY" | field status)" accepted
REPLY=$(broadcast "$CONFLICT")
check "链上查不到的双花仍然是拒绝" "$(echo "$REPLY" | field error.code)" broadcast_rejected

echo "== 历史"
REPLY=$(api_post /v1/$NETWORK/history "{\"addresses\":[\"$PAYEE\"],\"cursor\":null}")
echo "$REPLY" | python3 -m json.tool --no-ensure-ascii | head -24
check "历史有两笔" "$(echo "$REPLY" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["items"]))')" 2
check "转入金额" "$(echo "$REPLY" | python3 -c "
import json, sys
print([i['amount'] for i in json.load(sys.stdin)['items'] if i['to'] == '$PAYEE'][0])")" 50000000
check "转出金额" "$(echo "$REPLY" | python3 -c "
import json, sys
print([i['amount'] for i in json.load(sys.stdin)['items'] if i['from'] == '$PAYEE'][0])")" 49990000

echo
echo "通过 $PASSED 项，失败 $FAILED 项"
[[ "$FAILED" -eq 0 ]]
