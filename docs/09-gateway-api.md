# 网关接口格式

[系统设计 §4](03-system.md#4-网关) 定义接口列表,本文定义请求和响应格式.

## 通用约定

- 金额一律是最小单位的整数字符串,如 `"100000000"`,避免精度丢失.
- 错误响应:HTTP 状态码加 `{"error": {"code": "invalid_address", "message": "地址格式错误"}}`.
- 错误码:`unknown_network`,`invalid_address`,`too_many_addresses`,`upstream_unavailable`,`rate_limited`,`invalid_request`,`broadcast_rejected`.
- `broadcast_rejected` 用 HTTP 422.哈希格式或数量错误是 `invalid_request`.
- 单次请求最多 100 个地址.

## GET /v1/version

```json
{ "version": "0.1.0", "networks": ["bitcoin", "ethereum"], "noHistory": ["bsc"] }
```

只返回配置了上游的网络.Chain ID 等信息内置在网页端,这里不提供.

## POST /v1/snapshot

一次取回多个网络的最新状态.响应是一行一个 JSON,哪批先查完就先发,慢的网络不拖累其他网络,一个网络失败也不影响其他网络.

```json
{
    "items": [
        { "network": "ethereum", "address": "0x...", "since": "20000000", "group": 0 },
        { "network": "ethereum", "address": "0x...", "contract": "0x...", "since": "20000000", "group": 0 },
        { "network": "bitcoin", "address": "bc1...", "group": 1 }
    ]
}
```

没有 `contract` 时是原生币.`since` 是这批地址上次的 `checkpoint`.`group` 是客户端的分批编号,同一网络,同一 `since`,同一 `group` 的条目一起查询,一组最多 100 个地址.一次最多 1000 个条目.每组消耗一个限流令牌,不够时整个请求返回 `rate_limited`.

```text
{"network":"ethereum","addresses":["0x..."],"height":20000100,"balances":[{"address":"0x...","asset":"native","amount":"1"}],"history":{"items":[],"cursor":null,"checkpoint":"20000040","unchanged":false}}
{"network":"bitcoin","addresses":["bc1..."],"error":"upstream_unavailable"}
```

`height` 在查询之前取,之后的变化都在它之上.`balances` 只含请求的资产,`history` 的格式与 `/history` 相同,翻页和增量都接着用 `/history`.余额或历史没查到时对应字段为 `null`,没有索引服务的网络 `history` 也是 `null`.

## POST /v1/{network}/balances

请求:

```json
{ "addresses": ["0x9858EfFD232B4033E47d90003D41EC34EcaEda94"] }
```

响应:

```json
{
    "balances": [
        { "address": "0x9858EfFD232B4033E47d90003D41EC34EcaEda94", "asset": "native", "amount": "1000000000000000000" },
        {
            "address": "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
            "asset": "0xdAC17F958D2ee523a2206206994597C13D831ec7",
            "amount": "2500000"
        }
    ]
}
```

- `asset` 是 `native` 或合约地址.
- Bitcoin 余额只算已上链的部分,不含内存池,与其他链一致.
- 代币只返回网关配置的清单里的合约.网页端拿到后仍要按内置清单核对,清单外的显示为不在内置清单.

## POST /v1/{network}/used

Bitcoin.请求 `{"addresses": [...]}`,响应 `{"used": [...]}`,是这批地址里有过交易的,包含内存池.地址发现用.

## POST /v1/{network}/utxos

Bitcoin.请求 `{"addresses": [...]}`.

```json
{ "utxos": [{ "address": "bc1q...", "txid": "...", "vout": 0, "amount": "50000", "height": 840000 }] }
```

`height` 为 0 表示还在内存池.

## POST /v1/{network}/raw-txs

Bitcoin.请求 `{"txids": [...]}`,最多 100 个.

```json
{ "txs": { "<txid>": "<hex>" } }
```

网页端要自己重算 txid.冷钱包也会校验,网关给错只会让交易被拒绝.

## POST /v1/{network}/nonce

EVM.请求 `{"addresses": [...]}`.

```json
{ "nonces": { "0x...": { "latest": 12, "pending": 13 } } }
```

## POST /v1/{network}/resources

TRON.请求 `{"addresses": [...]}`.

```json
{ "resources": { "T...": { "bandwidth": 600, "energy": 0, "activated": true } } }
```

## GET /v1/{network}/ref-block

TRON.返回最新的已确认区块.

```json
{ "refBlockBytes": "ab12", "refBlockHash": "0011223344556677", "timestamp": 1790000000000 }
```

## POST /v1/{network}/nonce-account

Solana.请求 `{"address": "..."}`,地址是 nonce 账户.

```json
{ "exists": true, "nonce": "<Base58>", "lamports": "1447680" }
```

不存在时 `{"exists": false}`,账户存在但不是 nonce 账户时也一样.

## GET /v1/{network}/blockhash

Solana.只用于创建 nonce 账户.`{"blockhash": "<Base58>"}`

## GET /v1/{network}/fees

形式按链不同,见[系统设计 §10](03-system.md#10-手续费).

```json
{"kind": "bitcoin", "slow": 1, "medium": 3, "fast": 8}
{"kind": "evm", "baseFee": "20000000000", "slow": {"maxFee": "22000000000", "priority": "100000000"}, "medium": {}, "fast": {}}
{"kind": "tron", "energyPrice": 420, "bandwidthPrice": 1000}
{"kind": "solana", "slow": 0, "medium": 1000, "fast": 100000}
```

Bitcoin 单位是 sat/vB,EVM 是 wei,TRON 是 sun,Solana 是 micro-lamports 每计算单元.`medium`,`fast` 与 `slow` 结构相同.

- Bitcoin:目标 1,6,144 块,向上取整,下限 1.
- EVM:最高单价是基础费的 1.25,1.5,2 倍加优先费.
- Solana:中档下限 1000,快档下限 100000.

## POST /v1/{network}/estimate

请求和响应按链不同.

```json
// EVM
{"from": "0x...", "to": "0x...", "value": "0", "data": "0x..."}
{"gas": "65000"}

// TRON
{"from": "T...", "to": "T...", "contract": "T...或空", "data": "<hex>或空"}
{"energy": "65000", "bandwidth": "345", "activationFee": "0"}

// Solana
{"recipient": "...", "mint": "...或空"}
{"recipientTokenAccount": true, "rent": "0"}
```

合约回滚时返回 400 `invalid_request` 加节点原文.Solana 原生币转账返回 `true` 和 `"0"`.

## POST /v1/{network}/broadcast

请求 `{"raw": "<hex>"}`,可以带 `0x`.TRON 是完整 Transaction 的 protobuf,Solana 是线上格式.

```json
{ "hash": "...", "status": "accepted" }
```

节点拒绝时返回错误码 `broadcast_rejected`,`message` 带节点原文.节点拒绝后网关会按哈希查一次链上,已经在内存池或已上链的交易按成功处理,保证幂等.只在连接失败时换上游,超时返回 `upstream_unavailable`.网页端不能把这个错误当成定论,见[系统设计 §7](03-system.md#7-交易生命周期).

## POST /v1/{network}/tx

请求 `{"hashes": [...]}`,最多 100 个.

```json
{ "txs": [{ "hash": "...", "status": "included", "confirmations": 3, "stage": "safe", "block": 123, "fee": "21000" }] }
```

- `status`:`unknown`,`pending`,`included`,`finalized`,`failed`.`unknown` 和 `pending` 时 `stage`,`block`,`fee` 为 null,`confirmations` 为 0.
- `stage`:EVM 是 `latest`,`safe`,`finalized`,Solana 是 `processed`,`confirmed`,`finalized`,TRON 是 `included`,`finalized`,Bitcoin 为空,看 `confirmations`.
- `failed` 表示上链但执行失败,手续费已扣除.`failed` 优先于阶段.Polygon 部分节点不支持 `safe`,这个阶段会被跳过.
- Solana 的 `block` 是 slot,`fee` 可能为 null.
- 完成标准见[系统设计 §9](03-system.md#9-有效期与最终性),由网关按链判断.

## POST /v1/{network}/history

请求 `{"addresses": [...], "cursor": null, "since": null}`.`since` 是上次返回的 `checkpoint`,带上后只返回这之后的记录.

```json
{
    "items": [
        {
            "hash": "...",
            "time": 1790000000,
            "from": "...",
            "to": "...",
            "asset": "native",
            "amount": "100",
            "status": "finalized"
        }
    ],
    "cursor": null,
    "checkpoint": "19000000",
    "unchanged": false
}
```

Bitcoin 的条目多一个 `flows`:`{"inputs": ["..."], "outputs": [{"address": "...", "amount": "100"}]}`,数据输出的 `address` 是 `null`.`from`,`to`,`amount` 只按这次请求的地址归纳,钱包的地址分在几批时不准,客户端要用 `flows` 按整个钱包归纳.输入输出合计超过 256 个时没有这个字段.

`checkpoint` 是已经不可回滚的位置,也是不透明字符串,每页都返回,保存第一页的.`unchanged` 为真表示上次之后没有变化,这一页是空的,不能当成记录被回滚.只有 Bitcoin 会返回真,这时网关没有查询任何地址.

`cursor` 为空表示没有更多,它是不透明字符串.空页也可能带 `cursor`,不能遇到空页就停.`asset` 的写法和余额接口相同.`time`,`from`,`to` 可能为 null.

EVM 的历史查询依赖配置的 `indexer`(Etherscan 兼容接口),没配时返回 `upstream_unavailable`,不返回空列表.Bitcoin 的条目按一批地址归纳,同一笔交易里第二个自有输出不会出现在 `from` 和 `to` 里,地址发现不能只靠这两个字段.

## GET /v1/prices

```json
{ "prices": { "ethereum:native": "3400.12", "ethereum:0xdAC17F958D2ee523a2206206994597C13D831ec7": "1.00" } }
```

键是 `网络:合约`,和内置清单一致,不按符号.只有主网资产.测试网资产由网页端按内置清单里同链同符号的主网资产计价.价格只是展示,不参与任何判断.

## GET /v1/ws

JSON-RPC 2.0.`addresses` 频道:

```json
{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "subscribe",
    "params": { "channel": "addresses", "network": "ethereum", "addresses": ["0x..."] }
}
```

网关先取好基线和当时的高度,再回复 `{"jsonrpc": "2.0", "id": 1, "result": "<订阅 ID>"}`.有新高度时推送:

```json
{
    "jsonrpc": "2.0",
    "method": "update",
    "params": {
        "subscription": "<订阅 ID>",
        "height": 20000101,
        "previous": 20000100,
        "balances": [{ "address": "0x...", "asset": "native", "amount": "1" }],
        "txs": []
    }
}
```

`balances` 只含余额变了的地址,是绝对值.`txs` 的格式与 `/history` 的条目相同,地址没有变化时为空数组,网关这次没取到,或者上一条推送没发出去时为 `null`,这时客户端要自己补取.`previous` 是这个订阅上一条推送的高度,接不上说明漏了推送.Bitcoin 的内存池交易不改变高度,这时 `previous` 等于 `height`.

`ping` 方法返回 `"pong"`.浏览器看不到 WebSocket 的 ping 帧,要靠它发现连接已经断了.

订阅只存在于当前连接的内存里.Bitcoin 的余额只算已确认.单连接或单 IP 的地址数超过上限时返回 `too_many_addresses`.

`tx` 频道:

```json
{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "subscribe",
    "params": { "channel": "tx", "network": "ethereum", "hash": "0x..." }
}
```

状态变化时推送 `{"jsonrpc": "2.0", "method": "update", "params": {"subscription": "<订阅 ID>", "tx": {}}}`,`tx` 的格式与 `/tx` 接口相同.确认数变化只在 Bitcoin 上推送,到达最终状态后停止.

`blocks` 频道的参数是 `{"channel": "blocks", "network"}`,推送 `{"height": 123}`.`prices` 频道的参数是 `{"channel": "prices"}`,推送 `{"prices": {}}`.两者合计每个连接最多 18 个.所有推送都带 `subscription`.

WS 错误用 JSON-RPC 整数码,业务码放在 `error.data.code`.
