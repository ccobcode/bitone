# 验证状态

本文记录实现经过了哪些验证,依据是什么,还有哪些没有验证.

## 1. 依据的外部事实

| 事实                                                                            | 来源                                                                                                                                               |
| ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crypto-psbt` 已废弃,当前是 `psbt`                                              | [BCR-2020-006](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-006-urtypes.md)                                           |
| TRON 默认 60 秒过期,协议上限 24 小时                                            | [java-tron Constant.java](https://github.com/tronprotocol/java-tron/blob/master/common/src/main/java/org/tron/core/Constant.java)                  |
| Taproot 签名承诺全部输入金额                                                    | [BIP-341](https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki)                                                                          |
| SegWit 只承诺自身输入金额,存在超额手续费攻击                                    | [CVE-2020-14199](https://nvd.nist.gov/vuln/detail/CVE-2020-14199)                                                                                  |
| EIP-7702 的 `chain_id` 为 0 时对所有链生效                                      | [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702)                                                                                                |
| Polygon,BSC 支持 `finalized`                                                    | [Polygon 文档](https://docs.polygon.technology/pos/concepts/finality/finality),[BSC 文档](https://docs.bnbchain.org/bnb-smart-chain/introduction/) |
| TRON 区块确认需要 19 个超级代表                                                 | [TRON 文档](https://developers.tron.network/docs/tron-protocol-transaction)                                                                        |
| `setUserAuthenticationRequired` 的密钥在移除屏幕锁时永久失效                    | [Android 文档](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec.Builder)                                      |
| `setUnlockedDeviceRequired` 在 Android 12 到 14 上,移除屏幕锁时密钥会被自动删除 | 同上                                                                                                                                               |
| Solana blockhash 约 150 个 slot 过期,durable nonce 要求首条指令                 | [Chainstack](https://docs.chainstack.com/docs/solana-durable-nonces),第三方转述                                                                    |

## 2. 验证方式

`make test` 跑全部单元测试,构建和格式检查.

| 部分         | 方式                                                                                           |
| ------------ | ---------------------------------------------------------------------------------------------- |
| 助记词和派生 | BIP39,BIP84,BIP86,SLIP-0010 官方向量.TRON 和 Solana 地址与独立实现一致                         |
| Bitcoin      | BIP-143,BIP-341,BIP-174 向量.Bitcoin Core regtest 接受 SegWit,Taproot,混合,旧式,嵌套输入的交易 |
| EVM          | 编码和签名与 viem 逐字节一致.EIP-712 用规范示例.anvil 上走通广播,回滚和重组                    |
| TRON         | `raw_data`,txID 和 r,s 与 tronweb 一致.本地 java-tron 接受转账和 TRC-20                        |
| Solana       | 消息,签名和 ATA 与 web3.js 一致.本地 Agave 验证者走通 nonce 创建(含预存款场景),SOL 和 SPL 转账 |
| 网关         | 模拟上游加 anvil,bitcoind,java-tron.主网只读数据核对过状态和手续费                             |
| 网页端       | Chromium 端到端覆盖四条链的配对,构建,签名,核验,广播,取消和加速,严格 CSP 下无违规,无外部请求    |
| 冷钱包       | Android 模拟器走通扫码,核对,PIN 签名,响应通过 `tx::verify`.真实相机路径用虚拟场景验证过        |
| 构建         | 同一份代码关掉构建缓存,换目录从干净状态构建两次,开启 R8 的未签名 APK 哈希一致                  |
| 依赖         | `cargo audit`,`cargo deny`                                                                     |

## 3. 尚未验证

- 真机上的实体相机扫码,StrongBox,Android 12 到 14 的 Keystore 行为.
- 第三方安全审计.手写的编码逻辑只有向量和独立实现对比.
- PIN 错误计数的防回滚.Android 应用层没有查到通用接口,猜测速度靠硬件密钥按次认证限制.
- Argon2id 参数在低端手机上的耗时.
- 二维码静区,帧率和单帧容量的真机表现.
- durable nonce 的长期可用性,官方是否计划废弃没有核实.
- 跨机器和容器内的可复现构建.
- CI 和发布流程,没有实际运行过.
- Firefox 和 Safari.
- 网页端流程里的 TRC-20 和 SPL 上链,真实的 EVM 索引服务.

## 4. 已知限制

- 助记词显示和输入时,单词会以字符串形式经过 JVM,无法清除.PIN 和 BIP39 密码跨过绑定层时同样如此.其他时候密钥只留在 Rust 内存里.
- 熵和种子用 `zeroize` 清除.`rust-bitcoin` 的扩展私钥是可复制类型,派生过程中的中间私钥做不到完全清除.
- APK 未签名.
- debug 构建有开发标记,可以跳过 Root,Bootloader,调试三项检查,并放行软件密钥库.release 用另一份实现,不含这段代码.
- 助记词页的文字在无障碍树里可见,`FLAG_SECURE` 挡不住.靠自检拦截第三方无障碍服务和调试模式.
- Bootloader 状态来自密钥证明,没有校验证书链,只作自检参考.
- Web Locks 只在安全上下文可用,自部署用 HTTP 局域网地址时无法构建交易.
- 网关切换到备用上游后不会自动切回.`trusted_proxies` 只支持单个 IP,不支持 CIDR.
- EVM 的交易历史依赖 Etherscan 兼容的索引服务,JSON-RPC 查不了地址历史.
- 公共节点限流很紧,TronGrid 要配 API key.TRON 跟块一轮三次调用,一条网络全天约 4 万次,只在有订阅时运行.Solana 每个地址要几条订阅,公共节点的订阅上限没有公开.
- BSC,Polygon,BSC Testnet,Amoy 没有不用 key 的索引服务.HyperSync 免费额度是每分钟 15 次,所有网络和用户共用,它也看不到合约内部转入的原生币.
- EVM 跟块看不到合约内部转入的原生币,余额会更新,交易记录要等索引服务.
- 记录多到 HyperSync 一个请求扫不完时,最早的在前,中间的要翻页才看得到.
- 公共 Electrum 服务器由志愿者运营,不保证可用,能看到查询的地址.正式部署要用自建的 electrs 或 Fulcrum.
- Bitcoin 历史接口的条目按一批地址归纳,同一笔交易里第二个自有输出不会出现在 `from` 和 `to` 里,地址发现不能只靠这两个字段.
- ERC-7730 只实现了函数清单的最小子集.
- Solana 本地验证者的镜像要固定 `anzaxyz/agave:v2.3.13`,v3 在 Docker 转译下起不来.
