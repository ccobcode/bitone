# 交易解码标准

本文规定 BitOne 怎样解码待签名数据.Bitcoin,EVM,TRON,Solana 都按本文实现,手机和网页端共用核心库的同一份结果.

协议能承载什么,需求就是什么,和网页端当前发送什么无关.用户确认之前,必须能看到这笔交易的全部字段.

版本和上游状态核实于 2026-11-01.事实来源和假设分开列在第 9 节.

## 1. 范围边界

| 区域     | 定义                                      | 处理                                                           |
| -------- | ----------------------------------------- | -------------------------------------------------------------- |
| 协议层   | 字段名和类型只凭载荷和官方规范就能确定    | 100% 解码,没有十六进制回退                                     |
| 应用层   | 字段名和类型由第三方部署者定义,不在交易里 | 整段作为一个 `opaque` 字段归属字节,含义只来自第 7 节的描述文件 |
| 链上状态 | 需要的信息不在载荷里,离线设备看不到       | 不猜测,不采信网页端声明,策略按看不到处理                       |

**分界判断**:一个字段的名字和类型,能否只凭这笔载荷和官方规范得到.能就是协议层,要靠部署者提供就是应用层.

**没有十六进制回退**:协议层字段都按规范解码,不退化成显示十六进制.类型本身是字节串的字段(哈希,签名,证明,非 UTF-8 备注)用十六进制显示它的值,这些字段有名字,不算回退.本机不认识的格式不算解码失败,见第 1.1 节.

| 链      | 协议层                                                                | 应用层                                                                       | 链上状态                            |
| ------- | --------------------------------------------------------------------- | ---------------------------------------------------------------------------- | ----------------------------------- |
| EVM     | EIP-2718 信封的全部字段,访问列表,授权列表,EIP-712 的结构              | `data` 整体,包括创建合约的代码,预编译和系统合约的调用数据.EIP-712 的业务含义 | 合约代码,代币小数位,账户 nonce      |
| TRON    | `Transaction.raw` 和 41 个 `ContractType` 的全部消息                  | `TriggerSmartContract.data`,`CreateSmartContract.new_contract.bytecode`      | TRC-10 小数位,账户权限,动态费用参数 |
| Solana  | 消息(legacy,v0,v1),地址表引用,第 5.3 节列出的程序的指令数据和账户角色 | 其他程序的 `data` 和账户含义                                                 | 查找表里的地址,mint 的小数位和扩展  |
| Bitcoin | PSBT 全部键值,未签名交易,脚本操作码                                   | `OP_RETURN` 负载的含义                                                       | 除前序交易之外的 UTXO 状态          |

- SPL Token,Token-2022,Associated Token,Memo 是链上程序,不是共识规则.它们算协议层,因为格式由 Solana 官方定义,程序 ID 固定.判定只看程序 ID.
- ERC-20,TRC-20 是应用层.ABI 是约定,不是共识规则,节点不解释 `data`.

### 1.1 不限制原生行为

冷钱包可能多年不更新,今天的规则表不能替以后的用户做决定.协议允许的行为都可以签名,解码和规则只负责让用户看清内容,并用告警和确认方式提高门槛.

只有下面几种情况拒绝签名,它们都不是用户意图能表达的内容:

| 情况                 | 例子                                                                                                                         |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| 请求不属于本钱包     | 主密钥指纹不符                                                                                                               |
| 请求自相矛盾         | 派生路径和网络不符,地址和路径不符,交易的 Chain ID 和网络不符,前序交易和输入不符,`type_url` 和 `type` 不符,EIP-712 的隐藏成员 |
| 已知格式的字节不合规 | 非规范编码,多余字节,索引越界,这些内容链也不接受,不同解析器的结果可能不同                                                     |
| 签名无法正确完成     | 解锁后仍推导不出对应私钥,`SIGHASH_SINGLE` 没有对应输出,输入脚本不认识                                                        |
| 超过资源上限         | 整包,帧数和嵌套深度超过解析上限                                                                                              |

其余内容都可以签:

- 未来的交易类型,合约类型,指令和程序:能解码的外层字段照常解码,其余作为 `opaque` 节点显示原始数据.
- 已知的签名域先按真实含义解码,不算未知格式:`0x19` 是 EIP-191 和 EIP-712,`0x05` 是 EIP-7702 授权,Solana 的 `0xff` 开头是链下消息.
- 其余载荷都不认识时,树只有一个 `opaque` 根节点.未知的 EIP-2718 类型按 `keccak256(payload)` 计算签名原文.
- 未知网络:比如本地调试链或以后的新链.页面显示请求里的网络名,类别是无法识别,只要不能确认是测试网就显示主网提示.路径和网络的矛盾只对本机认识的网络检查,EVM,TRON 请求的路径例外,见下面的派生路径.Bitcoin 的 `paths` 是空的,所以看 `chain` 字段,没有时看 PSBT 魔数.TRON 看 `chain` 字段,没有时看 coin type `195'` 和载荷.
- EVM 请求的网络名本机不认识,Chain ID 本机认识时,以 Chain ID 为准,请求里的网络名不能改变显示,告警写明这一点.两边都不认识时显示请求里的网络名和 Chain ID.
- 路径不能单独判定链:EVM 路径不区分主网和测试网,coin type `1'` 可能是 TRON,Solana 或 Bitcoin 测试网.`SignRequest` 增加可选的链字段,没有时才按路径和载荷推断,`1'` 要由载荷认出来.
- 危险但合法的行为:裸哈希签名,消息签名,账户委托,不绑定输出的 sighash,没有 Chain ID 的交易,都是无法识别,告警要说清后果.裸哈希和整段未知载荷在长按之后还要二次确认.`0x19 0x01` 摘要和本机不认识的 EIP-191 版本看不到内容,同样要二次确认.
- 内置清单缺失或损坏不影响签名,只影响名称和图标.
- 派生路径不设白名单,任意路径都可以花费和导入,页面显示路径并告警.例外是 EVM,TRON 请求落在本钱包其他链主网账户的用途和 coin type 上,比如 `m/84'/0'`,这是矛盾,网络名本机不认识时也拒绝.这些私钥上有真实资产,裸哈希和未知载荷能签下它们的交易,改个网络名不能绕过.Bitcoin 找零的路径和索引也不设上限,本机能重新算出地址就显示为本钱包地址,路径不常见或索引跳得太远时告警并要求长按.声明是本钱包却算不出同一地址时拒绝.保存的公钥算不出的硬化路径解锁后用私钥核对.
- Bitcoin 的见证输入没有前序交易时手续费无法确认,类别是无法识别,告警写明这一点.前序交易和输入不符时拒绝.

新内容永远不会默认变成普通转账,普通转账只能由规则显式给出.

### 1.2 二维码协议冻结

手机可能多年不更新,网页端和网关一直在变,所以二维码协议的 `version` 永远是 1,不再升级.

- 以后只增加可选字段.已有字段的编号,类型和含义不能修改,必填字段不能删除.
- 旧手机忽略不认识的字段,这些字段不能影响签名内容.
- 冻结的内容:UR 类型名,CBOR 键,链和账户类型编号,`kind` 取值,网络标识,签名布局,分帧方式,大小上限.

## 2. 统一解码模型

### 2.1 节点

四条链解码成同一棵树.

| 字段       | 含义                                                                                                         |
| ---------- | ------------------------------------------------------------------------------------------------------------ |
| `name`     | 官方定义的字段名,稳定不翻译,比如 `max_fee_per_gas`,`to_address`                                              |
| `kind`     | `bool`,`int`,`uint`,`amount`,`address`,`hash`,`bytes`,`text`,`enum`,`time`,`script`,`struct`,`list`,`opaque` |
| `value`    | 类型化的值.`struct`,`list` 没有值,只有子节点                                                                 |
| `source`   | 来源,见下表                                                                                                  |
| `children` | 子节点                                                                                                       |

| 来源         | 含义                                            | 带字节范围 | 能否作为策略依据 |
| ------------ | ----------------------------------------------- | ---------- | ---------------- |
| `signed`     | 载荷里进入签名原文的字节                        | 是         | 能               |
| `context`    | 载荷里不进入签名原文的字节,比如 PSBT 的派生路径 | 是         | 校验通过后能     |
| `derived`    | 由其他节点算出,比如手续费上限,恢复出的授权人    | 否         | 能               |
| `manifest`   | 已验签清单里的数据,比如代币符号                 | 否         | 能               |
| `descriptor` | 已验签描述文件里的数据                          | 否         | 只影响展示       |
| `hint`       | 网页端声明,未验证                               | 否         | 不能             |

`hint` 验证通过后变成 `derived`.比如 Solana 收款方所有者,要由所有者,mint,程序推导出目标账户才算通过.

`text` 的值里,控制字符,双向控制符和零宽字符都转义后显示.

### 2.2 全覆盖保证

载荷 P 按已识别的格式解码成树 T,下面四条全部成立才算成功.格式已识别而检查失败时拒绝,页面显示失败原因和 P 的 SHA-256.格式不认识时不走这四条,见第 1.1 节.

| 编号 | 条件                                                                                      | 检查方式                                  | 时机   |
| ---- | ----------------------------------------------------------------------------------------- | ----------------------------------------- | ------ |
| C1   | 划分:`signed`,`context` 叶子的字节范围加结构字节(标签,长度前缀,列表头)恰好覆盖 P,不重不漏 | 遍历树累加区间                            | 运行时 |
| C2   | 往返:由 T 重新编码,结果和 P 逐字节相同                                                    | 字节比较                                  | 运行时 |
| C3   | 穷尽:官方类型的每个字段和每个枚举分支都进入 T                                             | 生成代码,或解构不写 `..`,`match` 不写 `_` | 编译时 |
| C4   | 展示:T 的每个节点都进入渲染列表                                                           | 列表行数等于节点数,见第 4 节              | 运行时 |

C1 和 C2 都要,因为官方解码器成功不代表字节被用完:

- `prost` 跳过未知标签.
- `TokenInstruction::unpack` 丢弃尾部字节.
- `TxLegacy::decode` 丢弃 `chain_id` 之后的两项不检查,文档写明解码成功不代表规范的签名载荷.

官方解码器内部没有字节范围.C1 的范围精确到容器格式的字段,进入官方编解码器后子节点继承父节点的范围,这一段由 C2 证明.

### 2.3 各载荷的检查方式

| 载荷     | 签名原文                  | 结构遍历        | 字段含义来自                  | C2                                                   |
| -------- | ------------------------- | --------------- | ----------------------------- | ---------------------------------------------------- |
| EVM 交易 | 类型字节加 RLP            | RLP 项          | `alloy-consensus` 类型        | 官方编码.旧式交易另查 9 项形式,末两项必须为 0        |
| EIP-712  | 由类型算出的哈希          | JSON 成员       | `alloy-dyn-abi` 的 `Resolver` | `Resolver` 重新计算 `hashStruct`                     |
| TRON     | `raw_data` 字节           | protobuf 线格式 | 官方 proto 生成的类型         | `prost` 编码                                         |
| Solana   | 消息字节                  | 消息布局        | 官方 Codama IDL               | 按 IDL 重新编码                                      |
| Bitcoin  | 未签名交易加 sighash 原文 | PSBT 键值       | `bitcoin`,`psbt-v2` 类型      | 交易的共识编码.PSBT 映射内的键序不固定,这一层只用 C1 |

- PSBT v2 没有交易字节,C2 比较遍历器读到的字段和官方解码结果拼成的交易.遍历器和官方解码器读到的条目数也要相同.
- EIP-712 的载荷是 JSON,字节本身不进签名原文.C1 改为:顶层只有 `types`,`primaryType`,`domain`,`message`,每个 JSON 成员都被类型定义消费,重复键和多余成员都失败.`serde_json` 对重复键保留最后一个,所以 `decode/typed.rs` 用自己的 `Visitor` 收集.小数和超过 64 位的 JSON 数字不同解析器取值不同,也失败,这些值要写成字符串.
- EIP-712 的结构由 `decode/typed.rs` 按请求里的 `types` 遍历,原子类型用 `DynSolType::coerce_json`,树和摘要来自同一份 `DynSolValue`.不用 `TypedData::eip712_signing_hash`:它按固定字段哈希域,不看请求声明的 `EIP712Domain`,和 viem,MetaMask 不同,而且不接受自引用类型.`types` 没有 `EIP712Domain` 时按 viem 的规则从域的成员推出.
- EIP-712 的 C2 比较 `Resolver::eip712_data_word` 和遍历器算出的域,消息的哈希.`Resolver` 表示不了两个以上类型互相引用,这种消息只有遍历器一份实现,靠 viem 的向量验证.`Resolver` 不认识 `trcToken`,比较时类型串换成 `uint256`.
- TRON 节点用 `getRawData().toByteArray()` 重新序列化 `raw`,`Contract`,`Any` 后取哈希.这三层字段乱序或显式写默认值的载荷,哈希和节点算出的不同.
- `Any.value` 的字节节点原样保留,链接受这一层的非规范写法.这里仍然拒绝:重复的标量在不同解析器上取值不同,官方类型重新编码也回不到原文,C2 不成立.
- TRON 的未知字段和 `map` 不进官方类型的比较.未知字段是 `opaque` 节点,字节就是载荷本身.`map` 的条目 `prost` 跳过默认值,Java 和 tronweb 都写出键和值,tronweb 的键按字符串排序,所以这一段由遍历器负责.其余字节都和官方类型的编码比较.
- 解析前的大小和嵌套限制沿用 `check_limits`.

### 2.4 手写范围

本仓库只写格式级遍历器:RLP 项,protobuf 线格式,Solana 消息布局,PSBT 键值,Codama 类型节点.类型级知识都来自官方 schema 或官方类型.

- 不按手算偏移取字段,不抄写标签号和判别值,这些都来自 schema.
- 新增合约类型或指令,改的是 schema 数据和规则表,不是解析代码.
- 协议数据不在脚本里切片.

## 3. 解码和策略分离

```text
decode(network, kind, payload) -> Decoded
assess(Decoded, manifest, keys, highest_change) -> Assessment
```

`decode` 是纯函数,不读清单和密钥.`assess` 不修改树,只按节点 ID 追加发现:风险,告警,展示值,本账户标记.

请求信封的检查顺序不变,见[系统设计 §6](03-system.md).网络和派生路径不在载荷里,作为 `derived` 节点进树.Bitcoin 的路径在 PSBT 里,请求的 `paths` 没有校验,不进树.

1. 策略只读树,拿不到原始载荷.`crates/bitone-core/tests/architecture.rs` 检查 `policy` 只依赖模型,`decode` 不依赖 `policy`,`manifest`,`review`.
2. 规则是白名单.规则表按节点种类给类别,种类名来自 schema.没有规则的节点是无法识别,普通转账只能由规则显式给出.
3. 没有动作的交易是无法识别.
4. 一笔交易取全部动作里最高的类别.告警只升级确认方式,不降低类别.
5. 规则表是核心库内置的数据文件.清单和描述文件都不能修改规则,描述文件也不能降低类别.
6. CI 检查三方一致:官方枚举,第 5 节矩阵对应的规则表,语料样本.重新生成 schema 后出现新类型时 CI 失败,补上规则才能合并.
7. 第一屏也来自树和规则表.规则的 `summary` 写明动作的字段在哪里,`tx/summary.rs` 只按路径读树,不按指令或合约类型写代码.路径是节点名,用点分隔,`repeated` 字段的每一项都算.测试检查路径都在官方 schema 里.

| 顺序 | 类别     | 含义                                              | 确认                                                                        |
| ---- | -------- | ------------------------------------------------- | --------------------------------------------------------------------------- |
| 6    | 禁止     | 第 1.1 节列出的拒绝情况                           | 拒绝                                                                        |
| 5    | 无法识别 | 格式或含义不认识,没有规则,没有描述,或规则标为危险 | 长按,最强告警.规则的 `second` 为真和根节点是 `opaque` 时,长按后还要二次确认 |
| 4    | 权限变更 | 账户控制权改变                                    | 长按                                                                        |
| 3    | 授权     | 第三方可以动用资产                                | 长按                                                                        |
| 2    | 已识别   | 含义完整,不是普通转账                             | 点按,有告警时长按                                                           |
| 1    | 普通转账 | 白名单内的原生币和清单内代币转账                  | 点按,有告警时长按                                                           |

无法识别排在权限变更之上,一笔同时含授权和未知调用的交易按无法识别告警.

| `summary` 的键     | 含义                                                                   |
| ------------------ | ---------------------------------------------------------------------- |
| `amount`           | 金额                                                                   |
| `asset`,`decimals` | 金额的资产和指令写明的小数位.清单里有这个资产时显示符号,否则是原始整数 |
| `unlimited`        | 金额是 64 位最大值时表示无限额度                                       |
| `party`,`label`    | 对方和它的标签.有多个对方时都进 `parties`,顶部留空                     |
| `owner`            | `party` 是代币账户时的所有者,没有这一行就是所有者没有确认              |
| `next`,`current`   | 新控制方和当前控制方                                                   |
| `none`             | `next` 的值写明没有时的含义                                            |
| `holders`          | 新权限的全部持有人,有 `next` 之外的地址时告警                          |

TRON 的调用数据不在树的字段里,`TriggerSmartContract` 的金额和对方来自第 7 节.

## 4. 渲染和 FFI

### 4.1 FFI 形状

树在 FFI 上是前序遍历的扁平列表.UniFFI 0.32 支持递归记录,这里仍用列表,因为渲染本身是线性的,行数校验简单,WASM 的 serde 输出也是同一个结构.

```text
record FieldRow
  id: u32
  parent: u32?
  depth: u32
  name: string
  label: string
  kind: FieldKind
  value: string
  source: FieldSource
  offset: u32?
  len: u32?
  risk: RiskLevel?
  note: string?
  own: bool

record ReviewController
  current: string?
  next: string?

record ReviewParty
  label: string
  value: string

record ReviewSummary
  title: string
  amount: ReviewAmount?
  counterparty_label: string?
  counterparty: string?
  field_ids: list<u32>
  controllers: list<ReviewController>
  parties: list<ReviewParty>
  payer: string?

record ReviewResult
  risk: RiskLevel
  confirm: ConfirmMode
  network: string
  mainnet: bool
  summary: ReviewSummary
  alerts: list<string>
  fields: list<FieldRow>
  field_count: u32
  review_digest: bytes
  refusal: string?
  fingerprint_mismatch: bool
  request_fingerprint: string?
```

通用列表不按字段名分支,所以"谁是当前控制方"这类含义 `fields` 表达不了.`controllers`,`parties`,`payer` 只给 A6d 的控制方卡片,权限变更页第一屏的对方和 A8 的签名账户,页面的其余内容都来自 `fields`.网页端的 `previewRequest` 是同一个结构的 serde 输出.

权限变更都写进 `controllers`.`next` 为空表示这笔变更没有新控制方,卡片写明这一点,不拿 `counterparty` 顶替.

### 4.2 渲染规则

1. 手机只有一个通用列表,不按链,不按字段名分支.分组只看 `parent` 和 `depth`,`INPUT_LABELS`,`PARTY_LABELS`,`FIRST_SCREEN_LABELS` 这类按标签分组的逻辑不再存在.
2. `value` 由核心库按 `kind` 格式化,完整不截断.手机不做格式化.
3. `label` 来自核心库的数据文件,键是 `name`.CI 检查没有缺失,运行时缺失就显示 `name`.
4. 摘要在列表上方,不能代替列表.摘要的每个值都引用 `field_ids`,所以摘要不可能显示树里没有的内容.文字和某一行相同时引用那一行,算出来的值引用它的来源:Bitcoin 的发送额是外部输出的行,全部资产授权的"无限"是 `data` 这一行.有多个动作时顶部是唯一类别最高的动作,顺序在 `headline_rank`,无法识别排在授权之下.最高的不止一个,或者没有内容可放的动作不低于它时顶部留空.
5. `signed` 行不可折叠,`context` 行可以折叠.
6. 列表滚到底之前,确认控件不可用.
7. `field_count` 等于节点数.`review_digest` 是全部行,风险和确认方式的 SHA-256.手机把它,已渲染行数和二次确认的结果传回 `sign_request`,核心库重新解码算出相同结果才签名,要二次确认而没有确认时拒绝.

### 4.3 需要重新生成

| 产物        | 命令                | 随之修改                                                                                                         |
| ----------- | ------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Kotlin 绑定 | `make android-core` | `UniffiWalletCore.kt`,`platform/Review.kt`,`ReviewScreen.kt`,`reviewStyle`,测试里的 `Fakes.kt`,`SignFlowTest.kt` |
| `jniLibs`   | 同上                | 无                                                                                                               |
| WASM 和类型 | `make wasm`         | `apps/web/src/core/index.ts`,`chains/transfer.ts`,`pages/Sign.tsx`                                               |

绑定和 `jniLibs` 都被 Git 忽略,由 CI 生成.

## 5. 各链标准

矩阵里的"告警"表示这一行固定产生告警,因此长按.类别是策略规则表的内容.

### 5.1 EVM

交易类型按 EIP-2718 的类型字节分发.

| 类型 | 字段                                                                                                          | 解码类型    | 可签 | 类别                                        |
| ---- | ------------------------------------------------------------------------------------------------------------- | ----------- | ---- | ------------------------------------------- |
| 0x00 | `nonce`,`gas_price`,`gas_limit`,`to`,`value`,`data`,`chain_id`,0,0                                            | `TxLegacy`  | 是   | 按调用判定                                  |
| 0x00 | 同上,没有 `chain_id`                                                                                          | `TxLegacy`  | 是   | 无法识别,告警.签名后在每条 EVM 链上都能重放 |
| 0x01 | `chain_id`,`nonce`,`gas_price`,`gas_limit`,`to`,`value`,`data`,`access_list`                                  | `TxEip2930` | 是   | 按调用判定                                  |
| 0x02 | `chain_id`,`nonce`,`max_priority_fee_per_gas`,`max_fee_per_gas`,`gas_limit`,`to`,`value`,`data`,`access_list` | `TxEip1559` | 是   | 按调用判定                                  |
| 0x03 | 0x02 的全部字段加 `max_fee_per_blob_gas`,`blob_versioned_hashes`                                              | `TxEip4844` | 是   | 无法识别,告警.`to` 不能为空                 |
| 0x04 | 0x02 的全部字段加 `authorization_list`                                                                        | `TxEip7702` | 是   | 权限变更.`to` 和授权列表不能为空            |
| 0x06 | EIP-8141 帧交易,草案,未激活                                                                                   | 不解码      | 是   | 无法识别,二次确认                           |
| 其他 | 无                                                                                                            | 不解码      | 是   | 无法识别,二次确认                           |

类型字节的分发用 alloy 的 `TxType`,字段的名字和顺序在 `crates/bitone-core/assets/evm-schema.json`,值来自解构官方类型.每个字段都和官方类型的编码比较,schema 的顺序错了 C2 不通过.`SignedAuthorization` 的字段是私有的,靠 `Named::finish` 保证全部用完.

- **按调用判定**:`to` 有值且 `data` 为空是普通转账.`to` 有值且 `data` 非空按第 7 节,`data` 是 `bytes` 节点,描述文件解出的参数是它的子节点,来源是 `descriptor`,调用方法和合约写在这一行的备注里.`to` 为空是创建合约,无法识别.
- **Chain ID**:本机认识的网络和 Chain ID 不符是禁止.网络名本机不认识时,Chain ID 本机认识就显示对应的网络,否则显示请求里的网络名和 Chain ID,类别是无法识别,有主网提示.路径不分主网和测试网,主网提示只看 Chain ID.
- **路径**:不设白名单,见[安全设计 §2.6](04-security.md#26-派生路径校验).
- **未知类型**:树是一个 `opaque` 根节点,签名原文是 `keccak256(payload)`.字面的 0x00 和 0x80 到 0xbf 开头的载荷也走这里.
- **手续费上限**是 `derived` 节点,类型 0x03 要加上 blob 费.
- **blob**:内容不在交易里,只有版本化哈希,所以固定告警.`assemble` 输出共识形式,边车由广播方附上.
- **宽度**:alloy 的费用字段是 `u128`,`nonce`,`gas_limit`,`chain_id` 是 `u64`.超过这些宽度的值解码失败.ethereum/tests 的 `V_equals38` 属于这种情况,费用超过 128 位.
- **装回签名**:类型 0 到 4 由 alloy 编码,没有 `chain_id` 的旧式交易 `v` 是 27 或 28,有 `chain_id` 时是 `chain_id * 2 + 35 + y_parity`.未知类型和消息没有可以广播的形式,`verify` 只返回核验过的签名,`signature_only` 为真.

| 结构                 | 字段                                                                    | 规则                                                                                                                                                              |
| -------------------- | ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `access_list`        | 每项 `address`,`storage_keys`                                           | 全部列出,不只显示数量                                                                                                                                             |
| `authorization_list` | 每项 `chain_id`,`address`,`nonce`,`y_parity`,`r`,`s`,派生的 `authority` | 全部列出,不能为空.`authority` 由签名恢复,高 s 算恢复失败.`chain_id` 为 0,和网络不符,或恢复失败都是无法识别,只显示不拒绝.`address` 为零地址是清除委托,仍是权限变更 |

| 消息                                                            | 解码                                     | 可签 | 类别              |
| --------------------------------------------------------------- | ---------------------------------------- | ---- | ----------------- |
| EIP-712 Permit,Permit2,类型串逐字相符                           | 全部字段                                 | 是   | 授权              |
| EIP-712 任意消息,有描述文件                                     | 同上,按描述展示                          | 是   | 已识别            |
| EIP-712 任意消息,没有描述文件                                   | 同上,`domain`,`types`,`message` 全部显示 | 是   | 无法识别          |
| EIP-712 域缺 `chainId` 或 `verifyingContract`,或 `chainId` 不符 | 同上                                     | 是   | 无法识别,告警     |
| `personal_sign`(EIP-191 版本 0x45)                              | 文本或字节                               | 是   | 无法识别          |
| EIP-191 版本 0x00                                               | 验证者地址,数据                          | 是   | 无法识别          |
| EIP-191 其他版本                                                | 版本,`opaque` 数据                       | 是   | 无法识别,二次确认 |
| `0x19 0x01` 加域分隔符和消息哈希                                | `domain_separator`,`hash_struct`         | 是   | 无法识别,二次确认 |
| `eth_sign`                                                      | 32 字节哈希,没有内容可解码               | 是   | 无法识别,二次确认 |
| EIP-7702 授权元组(前缀 0x05)                                    | `chain_id`,`address`,`nonce`             | 是   | 无法识别          |

- EIP-712 的 `chainId` 和网络不符不算矛盾:这个签名在 `chainId` 的链上有效,告警写明这一点.只有 `chainId` 和本机认识的测试网相符时才去掉主网提示.本机不认识的网络名按 `chainId` 显示,规则同交易.
- `chainId` 不符的 Permit 是无法识别,页面照常显示被授权方和额度.
- 类型里没有声明的成员不进哈希,页面上的值和签名无关,这是矛盾,拒绝.类型不是 `uint256` 的 `chainId`,类型不是 `address` 的 `verifyingContract` 不算绑定.

`SignRequest.kind`:0 是交易和按载荷分辨的签名域,1 是 EIP-712 的 JSON,2 是 32 字节的裸哈希,3 是 `personal_sign` 的消息.其他取值不知道怎么算签名原文,拒绝.kind 0 的 `0x19`,`0x05` 载荷的签名原文是 `keccak256(payload)`,不符合对应格式时拒绝,不走未知类型的路径.

### 5.2 TRON

依据是 java-tron 的官方 proto,固定在 tag `GreatVoyage-v4.8.2.4`.全部消息由生成代码解码,每个字段都进树.

| 编号 | `Transaction.raw` 字段 | 规则                                                                                 |
| ---- | ---------------------- | ------------------------------------------------------------------------------------ |
| 1    | `ref_block_bytes`      | 2 字节                                                                               |
| 3    | `ref_block_num`        | 解码并显示,非 0 是无法识别                                                           |
| 4    | `ref_block_hash`       | 8 字节                                                                               |
| 8    | `expiration`           | 不在 `timestamp` 之后的 24 小时内时告警.节点比较的是区块时间,离线设备没有时钟,不拒绝 |
| 9    | `auths`                | 解码并显示,非空是无法识别                                                            |
| 10   | `data`                 | 备注.合法 UTF-8 是 `text`,否则是 `bytes`.非空时节点按 `getMemoFee` 额外扣费,所以告警 |
| 11   | `contract`             | 恰好 1 个,节点也这样要求                                                             |
| 12   | `scripts`              | 解码并显示,非空是无法识别                                                            |
| 14   | `timestamp`            | 时间                                                                                 |
| 18   | `fee_limit`            | 金额,超阈值告警                                                                      |

| 编号 | `Contract` 字段 | 规则                                                                                                                                            |
| ---- | --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | `type`          | 枚举.未知编号是未来的合约类型,`parameter.value` 作为 `opaque`,长按后二次确认                                                                    |
| 2    | `parameter`     | `type_url` 必须是 `type.googleapis.com/protocol.` 加和 `type` 相符的消息名.和 `type` 不符是矛盾,拒绝.未知编号的 `type_url` 不能是已知合约的名字 |
| 3    | `provider`      | 解码并显示,非空是无法识别                                                                                                                       |
| 4    | `ContractName`  | 解码并显示,非空是无法识别                                                                                                                       |
| 5    | `Permission_id` | 解码并显示,非 0 时告警                                                                                                                          |

- 发起方字段按名字取 `owner_address`,和 java-tron 的 `getOwner` 一致,不再维护字段编号表.`ShieldedTransferContract` 取 `transparent_from_address`,可以为空.
- `owner_address`:`Permission_id` 为 0 时必须是本账户,否则签名无法完成,拒绝.`Permission_id` 非 0 时可以是其他账户,这是 TRON 的多签,类别是无法识别,告警写明花费的是谁的账户.保存的公钥算不出的路径解锁后核对.
- `int32`,`int64` 字段出现负数是无法识别.
- 未知字段:已知消息里的未知字段是 `opaque` 节点,名字是 `field_` 加编号,类别是无法识别,长按后可以签.以后的 java-tron 可能加字段.任何线格式类型都接受.
- 标签和长度必须是最短编码,字段按编号递增,未知字段也一样,非 `repeated` 字段不能重复.今天的节点把未知字段移到已知字段之后,排在中间的未知字段算出的哈希不同,这样的交易上不了链,不影响资金.
- 地址字段必须是 `0x41` 开头的 21 字节,`ref_block_bytes`,`ref_block_hash`,`operations` 长度不符也拒绝,链都不接受.`SmartContract.contract_address` 链不检查,按 `bytes` 显示.
- 字段的名字,编号,类型来自官方 proto,显示方式和动作在 `schemas/tron-fields.json`,类别和告警在 `assets/rules.json`.
- 编号 20,32,51 是原表缺少的类型.

| 编号 | 合约类型                        | 类别             | 说明                                                                                           |
| ---- | ------------------------------- | ---------------- | ---------------------------------------------------------------------------------------------- |
| 0    | AccountCreateContract           | 已识别           |                                                                                                |
| 1    | TransferContract                | 普通转账         |                                                                                                |
| 2    | TransferAssetContract           | 普通转账或已识别 | 清单内的 TRC-10 是普通转账.`asset_name` 只和清单的 TRC-10 条目比较.清单外告警,金额显示原始整数 |
| 3    | VoteAssetContract               | 无法识别         | 有消息定义,没有执行器                                                                          |
| 4    | VoteWitnessContract             | 已识别           | 列出全部投票对象                                                                               |
| 5    | WitnessCreateContract           | 已识别,告警      | 扣账户升级费                                                                                   |
| 6    | AssetIssueContract              | 已识别,告警      | 扣发行费                                                                                       |
| 8    | WitnessUpdateContract           | 已识别           |                                                                                                |
| 9    | ParticipateAssetIssueContract   | 已识别,告警      | 用 TRX 换 TRC-10                                                                               |
| 10   | AccountUpdateContract           | 已识别           |                                                                                                |
| 11   | FreezeBalanceContract           | 已识别           | `receiver_address` 不是本账户时告警                                                            |
| 12   | UnfreezeBalanceContract         | 已识别           | 同上                                                                                           |
| 13   | WithdrawBalanceContract         | 已识别           |                                                                                                |
| 14   | UnfreezeAssetContract           | 已识别           |                                                                                                |
| 15   | UpdateAssetContract             | 已识别           |                                                                                                |
| 16   | ProposalCreateContract          | 已识别           | `parameters` 是 map,按线上顺序显示.键重复,缺键或缺值时拒绝                                     |
| 17   | ProposalApproveContract         | 已识别           |                                                                                                |
| 18   | ProposalDeleteContract          | 已识别           |                                                                                                |
| 19   | SetAccountIdContract            | 已识别           |                                                                                                |
| 20   | CustomContract                  | 无法识别         | 只有枚举值,没有消息和执行器                                                                    |
| 30   | CreateSmartContract             | 无法识别         | `bytecode` 是应用层.其余字段,包括 `call_token_value`,全部解码                                  |
| 31   | TriggerSmartContract            | 按第 7 节        | `data` 是应用层.`call_value`,`call_token_value`,`token_id` 全部显示                            |
| 32   | GetContract                     | 无法识别         | 只有枚举值,没有消息和执行器                                                                    |
| 33   | UpdateSettingContract           | 已识别           |                                                                                                |
| 41   | ExchangeCreateContract          | 已识别,告警      | 资产转入交易对                                                                                 |
| 42   | ExchangeInjectContract          | 已识别,告警      | 同上                                                                                           |
| 43   | ExchangeWithdrawContract        | 已识别           |                                                                                                |
| 44   | ExchangeTransactionContract     | 已识别,告警      | 显示 `quant`,`expected`                                                                        |
| 45   | UpdateEnergyLimitContract       | 已识别           |                                                                                                |
| 46   | AccountPermissionUpdateContract | 权限变更         | 全部 `Permission` 和 `Key` 展开,`operations` 按合约类型编号解码位图                            |
| 48   | ClearABIContract                | 已识别           |                                                                                                |
| 49   | UpdateBrokerageContract         | 已识别           |                                                                                                |
| 51   | ShieldedTransferContract        | 无法识别         | 冷钱包没有屏蔽密钥,`transparent_from_address` 可以为空                                         |
| 52   | MarketSellAssetContract         | 已识别,告警      | 挂单卖出资产                                                                                   |
| 53   | MarketCancelOrderContract       | 已识别           |                                                                                                |
| 54   | FreezeBalanceV2Contract         | 已识别           |                                                                                                |
| 55   | UnfreezeBalanceV2Contract       | 已识别           |                                                                                                |
| 56   | WithdrawExpireUnfreezeContract  | 已识别           |                                                                                                |
| 57   | DelegateResourceContract        | 授权             | 显示 `receiver_address`,`lock`,`lock_period`                                                   |
| 58   | UnDelegateResourceContract      | 已识别           |                                                                                                |
| 59   | CancelAllUnfreezeV2Contract     | 已识别           |                                                                                                |

TRON 的消息和裸哈希都可以签,结果只有签名,`signature_only` 为真.路径规则和交易相同,`owner_address` 的检查不适用.

| `SignRequest.kind` | 标准    | 签名原文                                                | 类别              |
| ------------------ | ------- | ------------------------------------------------------- | ----------------- |
| 3                  | TIP-191 | `keccak256("\x19TRON Signed Message:\n" + 长度 + 消息)` | 无法识别          |
| 1                  | TIP-712 | 同 EIP-712,差别见下                                     | 同第 5.1 节的消息 |
| 2                  | 无      | 32 字节的哈希                                           | 无法识别,二次确认 |

- tronweb 的 `hashMessage` 是 TIP-191 的实现,长度是消息字节数的十进制文本,和 EIP-191 只差前缀.
- TIP-712 的 `address` 去掉 `0x41` 后按 20 字节编码,JSON 里可以是 Base58,`41` 开头或 `0x` 开头的十六进制,页面显示 Base58.
- `trcToken` 是原子类型,按 `uint256` 编码,类型串里保留 `trcToken`,所以 `typeHash` 和写成 `uint256` 时不同.
- 域的 `chainId` 是 `block.chainid & 0xffffffff`.主网 728126428,Nile 3448148188,Shasta 2494104990.这些值和 `Network` 对不上时按第 5.1 节告警.
- tronweb 拒绝类型的循环引用,TIP-712 没有禁止,这里和 EIP-712 一样接受.tronweb 不看请求里的 `EIP712Domain`,只按固定顺序取域的成员,两者只有在域按规范顺序声明时才相同.
- 消息和裸哈希不绑定链,页面都有主网提示.TIP-712 的 `chainId` 和本机认识的测试网相符时才去掉.

### 5.3 Solana

| 消息     | 首字节       | 字段                                                                          | 规则                                                                                                                                                                                                                                                                                                             |
| -------- | ------------ | ----------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| legacy   | 最高位为 0   | 头部,账户表,`recent_blockhash`,指令                                           | 账户表不重复,索引不越界,和节点的 sanitize 一致                                                                                                                                                                                                                                                                   |
| v0       | 0x80         | legacy 的字段加 `address_table_lookups`                                       | 同上                                                                                                                                                                                                                                                                                                             |
| v1       | 0x81         | 头部,`TransactionConfigMask`,`lifetime_specifier`,地址,配置值,指令头,指令数据 | 未知掩码位之后的内容宽度未知,作为 `opaque`,长按后二次确认.没有查找表.优先费的两位只写一位时拒绝.上限 4096 字节,12 个签名,64 个账户,64 条指令,堆大小 32 KiB 到 256 KiB 且是 1 KiB 的倍数,超过时告警,这些数字以后可能放宽.ComputeBudget 指令不影响配置,手续费只按配置值算.`assemble` 的签名在消息之后,没有长度前缀 |
| 未知     | 0x82 到 0xfe | 无                                                                            | 未知格式,整段 `opaque`,长按后二次确认,签名原文是载荷本身                                                                                                                                                                                                                                                         |
| 链下消息 | 0xff         | `\xffsolana offchain` 头部,版本和消息字段                                     | 按消息解码,见下面的链下消息.头部完整但后面不合规时拒绝.没有这段头部时是未知格式                                                                                                                                                                                                                                  |

- **签名者**:签名私钥必须是消息要求的签名者,否则是地址和路径不符,拒绝.手续费支付者不是本账户时是无法识别,可以签.
- **路径**:不设白名单,全部硬化的路径都可以签,不是本机导出的账户时告警.SLIP-0010 的 ed25519 没有非硬化派生,所以有非硬化层级时拒绝.coin type `501'` 和 `1'` 的私钥不同,和本机认识的网络不符时拒绝.没有导出的路径算不出公钥,签名者解锁后用私钥核对.
- **网络**:本机不认识的网络名只告警,类别是无法识别,有主网提示,金额显示原始整数.`chain` 字段优先,没有时 coin type `501'` 是 Solana,其余全部硬化的路径看载荷是不是完整的 Solana 消息.`1'` 不只属于一条链,不能单独判定.
- **查找表**:表地址和索引全部解码,表里的地址是链上状态.账户位来自查找表时,节点是表地址加索引,不猜测地址.消息含查找表时是无法识别.
- **账户**:每个账户节点带 IDL 里的账户名,签名者和可写标记.账户数少于 IDL 必需账户时解码失败,程序本身不会执行.IDL 的 `remainingAccounts` 有名字,要求签名的一组只收签名者.多出的账户程序通常忽略,作为 `opaque` 节点显示,类别是无法识别.
- **尾部字节**:程序本身会忽略尾部字节,链接受这种交易,所以不拒绝.它们作为 `opaque` 节点显示,类别是无法识别,C1 照常成立.
- **数据不完整**:数据比 IDL 短,或枚举,option,布尔的标签不认识时,程序以后可能改变格式,所以整段 `data` 是 `opaque`,类别是无法识别.只有文本不是合法 UTF-8 时解码失败,程序一定拒绝.
- **第三方程序**:程序 ID 不在下表时,按第 7.3 节处理.
- **Token-2022**:mint 的扩展是链上状态,转账时的费用和 hook 看不到.清单条目的 `extensions` 声明过的 mint 才能是普通转账,其余的 `transfer`,`transferChecked`,`transferCheckedWithFee` 是无法识别,金额显示原始整数.
- **没有广播形式**:链下消息,未知版本和未知掩码位的交易,`verify` 只返回核验过的签名,`signature_only` 为真.

链下消息有三种官方格式,外层都是 16 字节的 `\xffsolana offchain` 和 1 字节版本.

| 版本 | 字段                                                 | 来源                                                  |
| ---- | ---------------------------------------------------- | ----------------------------------------------------- |
| 0    | 格式,长度,消息                                       | `solana-offchain-message`                             |
| 0    | 应用域,格式,签名者数,签名者,长度,消息                | `@solana/offchain-messages`,Solana 的链下消息签名提案 |
| 1    | 签名者数,签名者,消息.签名者排序且不重复,没有长度字段 | `@solana/offchain-messages`,sRFC 3                    |
| 其他 | 版本,`opaque` 数据.外层已知,所以不走 `opaque` 根节点 | 无,无法识别,二次确认                                  |

- 两种版本 0 的布局互不兼容,同时符合两种布局的字节不同解析器结果不同,拒绝.
- 格式字段是受限 ASCII,受限 UTF-8,扩展 UTF-8,长度上限以各自来源为准.超过上限,内容不符合格式,消息为空时都拒绝.
- 消息声明了签名者时,签名私钥必须在其中.

解码方式都是通用遍历器读官方 Codama IDL.指令名取自 IDL.IDL 在 `schemas/solana/`,`tools/crosscheck/codama/generate.mjs` 校验哈希后生成 `crates/bitone-core/assets/solana-idl.json`,只留下遍历器要的内容.消息层的数字和名字在 `crates/bitone-core/assets/solana-schema.json`,类别和告警在 `assets/rules.json`,规则的 `passive` 为真时不算动作.

| 程序                 | 程序 ID                                                                                                                                   | 指令数 | 格式来源                                                 |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ------ | -------------------------------------------------------- |
| System               | `11111111111111111111111111111111`                                                                                                        | 14     | `solana-program/system`                                  |
| SPL Token            | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`                                                                                             | 28     | `solana-program/token`                                   |
| Token-2022           | `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`                                                                                             | 100    | `solana-program/token-2022`                              |
| Associated Token     | `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`                                                                                            | 4      | `solana-program/associated-token-account`                |
| Compute Budget       | `ComputeBudget111111111111111111111111111111`                                                                                             | 5      | `solana-program/compute-budget`                          |
| Memo                 | `Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo`,`MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`,`Memo4c2pN8afCj432Lb7RMVKi9PbQnnW7ewFFaV3oAH` | 1      | `solana-program/memo`                                    |
| Stake                | `Stake11111111111111111111111111111111111111`                                                                                             | 18     | `solana-program/stake`,IDL 17 条,加已废弃的 `redelegate` |
| Vote                 | `Vote111111111111111111111111111111111111111`                                                                                             | 20     | 官方没有 IDL,见下                                        |
| Address Lookup Table | `AddressLookupTab1e1111111111111111111111111`                                                                                             | 5      | `solana-program/address-lookup-table`                    |

| 程序                 | 类别        | 指令                                                                                                                                                                                                                                                                                  |
| -------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| System               | 普通转账    | `transferSol`                                                                                                                                                                                                                                                                         |
| System               | 权限变更    | `assign`,`assignWithSeed`,`authorizeNonceAccount`                                                                                                                                                                                                                                     |
| System               | 已识别,告警 | `transferSolWithSeed`,`withdrawNonceAccount`,`createAccount`,`createAccountWithSeed`,`createAccountAllowPrefund`,`allocateWithSeed`.本钱包的 nonce 创建里证明过的 `allocateWithSeed` 不告警                                                                                           |
| System               | 无法识别    | `advanceNonceAccount` 不在第一条或 nonce 账户不属于本钱包.`initializeNonceAccount` 和给本钱包 nonce 地址的 `allocateWithSeed` 不属于完整的本钱包 nonce 创建                                                                                                                           |
| System               | 已识别      | `allocate`,`initializeNonceAccount`,`upgradeNonceAccount`.`advanceNonceAccount` 在第一条且属于本钱包时不算动作,只有这一条指令时是取消交易,告警                                                                                                                                        |
| SPL Token            | 普通转账    | `transferChecked`,mint 在清单内,小数位一致,账户恰好 4 个                                                                                                                                                                                                                              |
| SPL Token            | 授权        | `approve`,`approveChecked`                                                                                                                                                                                                                                                            |
| SPL Token            | 权限变更    | `setAuthority`                                                                                                                                                                                                                                                                        |
| SPL Token            | 已识别,告警 | `transfer`,清单外的 `transferChecked`,`mintTo`,`mintToChecked`,`burn`,`burnChecked`,`closeAccount`,`unwrapLamports`,`withdrawExcessLamports`                                                                                                                                          |
| SPL Token            | 已识别      | `initializeMint`,`initializeMint2`,`initializeAccount`,`initializeAccount2`,`initializeAccount3`,`initializeMultisig`,`initializeMultisig2`,`revoke`,`freezeAccount`,`thawAccount`,`syncNative`,`getAccountDataSize`,`initializeImmutableOwner`,`amountToUiAmount`,`uiAmountToAmount` |
| SPL Token            | 无法识别    | 小数位和清单不符的 `transferChecked`                                                                                                                                                                                                                                                  |
| SPL Token            | 取内部最高  | `batch`.内部每条指令按同一 IDL 解码.嵌套 `batch` 解码失败.内部指令的第一屏,事实,所有者确认,动作数和无限额度都和消息里的指令相同                                                                                                                                                       |
| Associated Token     | 已识别      | `create`,`createIdempotent`,`createWithArgs`.地址不是由所有者,mint,程序推导得出时是无法识别.空数据等于 `create`                                                                                                                                                                       |
| Associated Token     | 已识别,告警 | `recoverNested`                                                                                                                                                                                                                                                                       |
| Compute Budget       | 已识别      | `requestHeapFrame`,`setComputeUnitLimit`,`setComputeUnitPrice`,`setLoadedAccountsDataSizeLimit`.不算动作,进入手续费节点                                                                                                                                                               |
| Compute Budget       | 无法识别    | `requestUnits`,已废弃                                                                                                                                                                                                                                                                 |
| Memo                 | 已识别      | `addMemo`.非法 UTF-8 解码失败                                                                                                                                                                                                                                                         |
| Stake                | 权限变更    | `authorize`,`authorizeChecked`,`authorizeWithSeed`,`authorizeCheckedWithSeed`,`setLockup`,`setLockupChecked`                                                                                                                                                                          |
| Stake                | 已识别,告警 | `withdraw`,`initialize`,`initializeChecked`,`split`,`merge`,`moveStake`,`moveLamports`                                                                                                                                                                                                |
| Stake                | 已识别      | `delegateStake`,`deactivate`,`getMinimumDelegation`,`deactivateDelinquent`                                                                                                                                                                                                            |
| Stake                | 无法识别    | `redelegate`,已废弃,不在 IDL 里,命中时按 `opaque` 显示                                                                                                                                                                                                                                |
| Vote                 | 权限变更    | `authorize`,`authorizeChecked`,`authorizeWithSeed`,`authorizeCheckedWithSeed`,`updateValidatorIdentity`                                                                                                                                                                               |
| Vote                 | 已识别,告警 | `withdraw`,`initializeAccount`,`initializeAccountV2`,`updateCommissionCollector`,`depositDelegatorRewards`                                                                                                                                                                            |
| Vote                 | 已识别      | `updateCommission`,`updateCommissionBps`                                                                                                                                                                                                                                              |
| Vote                 | 无法识别    | `vote`,`voteSwitch`,`updateVoteState`,`updateVoteStateSwitch`,`compactUpdateVoteState`,`compactUpdateVoteStateSwitch`,`towerSync`,`towerSyncSwitch`                                                                                                                                   |
| Address Lookup Table | 已识别      | `createLookupTable`,`extendLookupTable`,`deactivateLookupTable`                                                                                                                                                                                                                       |
| Address Lookup Table | 已识别,告警 | `freezeLookupTable`,不可逆.`closeLookupTable`                                                                                                                                                                                                                                         |

把 SOL 或代币转给本机确认不了的账户,或者把控制权交给这样的账户时,金额和目标都在第一屏,类别不变,告警要求长按.

**Stake**:官方 IDL 去掉了指令里的系统变量账户,官方程序正在移除对它们的依赖,现有钱包的交易还带着它们.旧布局写在 `schemas/solana/stake-older.json`,来自 `solana-stake-interface` 4.4.0,它是最后一个带系统变量的版本,和 `@solana/web3.js` 1.99.0 的 `StakeProgram` 核对过.解码只看交易里的地址:旧布局的第一个系统变量所在的位置上是这个系统变量时按旧布局读,否则按 IDL 读.`hint` 和请求的其他字节都不参与.这个位置的地址来自查找表时看不出是哪种布局,账户都叫 `account`,不命名.两种布局除了系统变量逐个相同,所以规则表的路径都能用.

**Vote**:官方没有 IDL,管理类指令写在 `schemas/solana/vote.json`,字节和账户标记都和 `solana-vote-interface` 的构建函数比较.8 条共识投票指令用了 Codama 表达不了的变长编码,不进 IDL,命中时按 `opaque` 显示,类别是无法识别.这是本标准已知的缺口.

Token-2022 的 0 到 25 号,38,45,255 号和 SPL Token 同名指令规则相同,`initializeMintCloseAuthority` 是已识别.扩展指令:

| 前缀   | 扩展                    | 类别                 | 指令                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------ | ----------------------- | -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 26     | TransferFee             | 已识别               | `initializeTransferFeeConfig`,`harvestWithheldTokensToMint`,`setTransferFee`                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 26     | TransferFee             | 已识别,告警          | `withdrawWithheldTokensFromMint`,`withdrawWithheldTokensFromAccounts`                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 26     | TransferFee             | 同 `transferChecked` | `transferCheckedWithFee`,显示 `fee`                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| 27     | ConfidentialTransfer    | 无法识别             | `initializeConfidentialTransferMint`,`updateConfidentialTransferMint`,`configureConfidentialTransferAccount`,`approveConfidentialTransferAccount`,`emptyConfidentialTransferAccount`,`confidentialDeposit`,`confidentialWithdraw`,`confidentialTransfer`,`applyConfidentialPendingBalance`,`enableConfidentialCredits`,`disableConfidentialCredits`,`enableNonConfidentialCredits`,`disableNonConfidentialCredits`,`confidentialTransferWithFee`,`configureConfidentialTransferAccountWithRegistry` |
| 28     | DefaultAccountState     | 已识别               | `initializeDefaultAccountState`,`updateDefaultAccountState`                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 29     | 无                      | 已识别               | `reallocate`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 30     | MemoTransfer            | 已识别               | `enableMemoTransfers`,`disableMemoTransfers`                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 31     | 无                      | 已识别               | `createNativeMint`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 32     | NonTransferable         | 已识别               | `initializeNonTransferableMint`                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 33     | InterestBearing         | 已识别               | `initializeInterestBearingMint`,`updateRateInterestBearingMint`                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 34     | CpiGuard                | 已识别               | `enableCpiGuard`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 34     | CpiGuard                | 已识别,告警          | `disableCpiGuard`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 35     | PermanentDelegate       | 权限变更             | `initializePermanentDelegate`                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 36     | TransferHook            | 已识别               | `initializeTransferHook`                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| 36     | TransferHook            | 权限变更             | `updateTransferHook`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 37     | ConfidentialTransferFee | 无法识别             | `initializeConfidentialTransferFee`,`withdrawWithheldTokensFromMintForConfidentialTransferFee`,`withdrawWithheldTokensFromAccountsForConfidentialTransferFee`,`harvestWithheldTokensToMintForConfidentialTransferFee`,`enableHarvestToMint`,`disableHarvestToMint`                                                                                                                                                                                                                                  |
| 39     | MetadataPointer         | 已识别               | `initializeMetadataPointer`,`updateMetadataPointer`                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| 40     | GroupPointer            | 已识别               | `initializeGroupPointer`,`updateGroupPointer`                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 41     | GroupMemberPointer      | 已识别               | `initializeGroupMemberPointer`,`updateGroupMemberPointer`                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 42     | ConfidentialMintBurn    | 无法识别             | `initializeConfidentialMintBurn`,`rotateSupplyElgamalPubkey`,`updateConfidentialMintBurnDecryptableSupply`,`confidentialMint`,`confidentialBurn`,`applyConfidentialPendingBurn`                                                                                                                                                                                                                                                                                                                     |
| 43     | ScaledUiAmount          | 已识别               | `initializeScaledUiAmountMint`,`updateMultiplierScaledUiMint`                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 44     | Pausable                | 已识别               | `initializePausableConfig`,`pause`,`resume`                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 46     | PermissionedBurn        | 已识别               | `initializePermissionedBurn`                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 46     | PermissionedBurn        | 已识别,告警          | `permissionedBurn`,`permissionedBurnChecked`                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 46     | PermissionedBurn        | 无法识别             | `permissionedConfidentialBurn`                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 8 字节 | TokenMetadata           | 已识别               | `initializeTokenMetadata`,`updateTokenMetadataField`,`removeTokenMetadataKey`,`emitTokenMetadata`                                                                                                                                                                                                                                                                                                                                                                                                   |
| 8 字节 | TokenMetadata           | 权限变更             | `updateTokenMetadataUpdateAuthority`                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 8 字节 | TokenGroup              | 已识别               | `initializeTokenGroup`,`updateTokenGroupMaxSize`,`initializeTokenGroupMember`                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 8 字节 | TokenGroup              | 权限变更             | `updateTokenGroupUpdateAuthority`                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |

- 机密转账的金额是密文,用户无法知道内容,所以归为无法识别.字段仍按 IDL 解码.
- `setAuthority` 的 `authorityType`:SPL Token 有 4 种,Token-2022 有 18 种,都按 IDL 的枚举显示名字.
- Loader v3,Loader v4,Config,Ed25519,Secp256k1,Secp256r1 预编译,ZK ElGamal Proof 也是原生程序,按分界属于协议层.它们不在首批范围,落地前按第三方程序处理,类别是无法识别.这是已知缺口.

### 5.4 Bitcoin

| `PSBT_GLOBAL_VERSION` | 解码                | 规则                                                                            |
| --------------------- | ------------------- | ------------------------------------------------------------------------------- |
| 缺失或 0              | `bitcoin` 的 `Psbt` | 全局键 `UNSIGNED_TX` 必须存在                                                   |
| 2                     | `psbt-v2`           | 交易字段在各输入输出的键里,按 BIP-370 确定 locktime.`TX_MODIFIABLE` 非 0 时告警 |
| 其他                  | 解码失败            | 不知道怎么算 sighash,签名无法完成                                               |

| 映射 | 键(省略 `PSBT_GLOBAL_`,`PSBT_IN_`,`PSBT_OUT_` 前缀)                                                                                                                                                                                                           | 来源      | 处理                                         |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------- | -------------------------------------------- |
| 全局 | `UNSIGNED_TX`,`TX_VERSION`,`FALLBACK_LOCKTIME`,`INPUT_COUNT`,`OUTPUT_COUNT`,`TX_MODIFIABLE`                                                                                                                                                                   | `signed`  | 解码成交易字段                               |
| 全局 | `XPUB`,`VERSION`,`PROPRIETARY`                                                                                                                                                                                                                                | `context` | 解码并显示                                   |
| 全局 | `SP_ECDH_SHARE`,`SP_DLEQ`                                                                                                                                                                                                                                     | `context` | 无法识别,BIP-375 仍是草案                    |
| 输入 | `PREVIOUS_TXID`,`OUTPUT_INDEX`,`SEQUENCE`,`REQUIRED_TIME_LOCKTIME`,`REQUIRED_HEIGHT_LOCKTIME`,`SIGHASH_TYPE`,`REDEEM_SCRIPT`,`WITNESS_SCRIPT`,`WITNESS_UTXO`                                                                                                  | `signed`  | 解码.`witness_utxo` 的金额和脚本进入 sighash |
| 输入 | `NON_WITNESS_UTXO`                                                                                                                                                                                                                                            | `context` | 按交易解码,重算 txid,被花费的输出单独显示    |
| 输入 | `BIP32_DERIVATION`,`TAP_BIP32_DERIVATION`,`TAP_INTERNAL_KEY`,`TAP_MERKLE_ROOT`,`TAP_LEAF_SCRIPT`,`PARTIAL_SIG`,`TAP_KEY_SIG`,`TAP_SCRIPT_SIG`,`FINAL_SCRIPTSIG`,`FINAL_SCRIPTWITNESS`,`RIPEMD160`,`SHA256`,`HASH160`,`HASH256`,`POR_COMMITMENT`,`PROPRIETARY` | `context` | 解码并显示                                   |
| 输入 | `MUSIG2_PARTICIPANT_PUBKEYS`,`MUSIG2_PUB_NONCE`,`MUSIG2_PARTIAL_SIG`,`SP_ECDH_SHARE`,`SP_DLEQ`                                                                                                                                                                | `context` | 无法识别,冷钱包不实现这些协议                |
| 输出 | `AMOUNT`,`SCRIPT`                                                                                                                                                                                                                                             | `signed`  | 解码成交易字段                               |
| 输出 | `REDEEM_SCRIPT`,`WITNESS_SCRIPT`,`BIP32_DERIVATION`,`TAP_INTERNAL_KEY`,`TAP_TREE`,`TAP_BIP32_DERIVATION`,`DNSSEC_PROOF`,`PROPRIETARY`                                                                                                                         | `context` | 解码并显示                                   |
| 输出 | `MUSIG2_PARTICIPANT_PUBKEYS`,`SP_V0_INFO`,`SP_V0_LABEL`                                                                                                                                                                                                       | `context` | 无法识别                                     |
| 任意 | 其他键类型                                                                                                                                                                                                                                                    | `context` | `opaque`,无法识别                            |

- 键的编号,名字,格式和来源都在 `crates/bitone-core/assets/psbt-schema.json`,遍历器只认容器格式.
- 键序不固定.重复的键,非规范的长度,版本要排除的键,版本要求却缺失的键都解码失败.
- `PROPRIETARY` 的值是 `opaque`.`psbt-v2` 没有 MuSig2 和 `DNSSEC_PROOF` 的常量,这些键由 PSBT 键值遍历器命名.
- `psbt-v2` 0.3.0 和 BIP-370 有三处不符,PSBT 进入它之前处理.`INPUT_COUNT` 排在 `OUTPUT_COUNT` 之后时它报重复键,所以键先按编号排序.金额 0 和空脚本它按缺失处理,所以换成占位值,字段的值以遍历器为准.输出的脚本只取 `SCRIPT` 这个键,`REDEEM_SCRIPT`,`WITNESS_SCRIPT` 的取值格式相同,但不进交易.`SEQUENCE` 缺失时它取 0,BIP-370 规定是 final,所以交易由遍历器拼出.

| 交易字段    | 显示                                                                       | 规则                                                                              |
| ----------- | -------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `version`   | 整数和含义                                                                 | 1,2 正常.3 是 TRUC,告警.其他值告警                                                |
| `lock_time` | 0 表示无.小于 500000000 是区块高度,否则是 UTC 时间                         | 非 0 时显示含义.全部 `sequence` 都是 0xffffffff 时说明它不生效                    |
| `sequence`  | 每个输入一个.显示 final,可替换(BIP-125),相对锁定(BIP-68,块数或 512 秒单位) | `version` 不小于 2 且第 31 位为 0 时才按相对锁定解释                              |
| `inputs`    | `txid`,`vout`,金额,脚本类型,本账户标记                                     | 输入金额,混合输入,sighash 的规则见[安全设计 §2.7](04-security.md#27-交易风险识别) |
| `outputs`   | 金额,脚本,地址,本账户标记                                                  | 找零规则见[安全设计 §2.6](04-security.md#26-派生路径校验)                         |

手续费和费率是 `derived` 节点,放在树的最前面.手续费无法确认时这一行是无法识别,带告警.

| 输出脚本                      | 解码                                                       | 类别                                  |
| ----------------------------- | ---------------------------------------------------------- | ------------------------------------- |
| P2PKH,P2SH,P2WPKH,P2WSH,P2TR  | 地址                                                       | 普通转账                              |
| P2PK,裸多签                   | 公钥列表,没有地址形式                                      | 已识别,告警                           |
| P2A(BIP-433)                  | 锚点,任何人可花                                            | 已识别,告警                           |
| `OP_RETURN`                   | 操作码列表.推入数据是 `bytes`,合法 UTF-8 时再加一行 `text` | 已识别.金额非 0 时告警,这笔钱会被销毁 |
| 见证版本 2 到 16              | 版本和程序                                                 | 无法识别,这些版本没有激活             |
| 长度不合法的见证程序,其他脚本 | 操作码列表                                                 | 无法识别                              |

`redeem_script`,`witness_script` 同样解码成操作码列表.花费路径不是本钱包地址类型时,解码照常完成,策略按安全设计处理.

`decode` 不读密钥,树里的派生路径只是声明,都来自 PSBT.`tx/bitcoin.rs` 重新派生后把本账户事实交给 `policy::assess_bitcoin`,它算出命中了哪些种类,类别和告警都来自 `assets/rules.json`.`version`,`lock_time`,`sequence` 的含义是规则表里的 `note`.

## 6. 依赖

### 6.1 运行时依赖

列的含义:依赖量来自 `cargo tree -e normal`,是该组在目标平台上的 crate 总数,"新增"是当前 `Cargo.lock` 没有的 crate 数.这些数字没有编译过,见第 9 节.

| 链      | crate                                                         | 版本     | 许可              | wasm32-unknown-unknown                  | Android    | 依赖量                                                            |
| ------- | ------------------------------------------------------------- | -------- | ----------------- | --------------------------------------- | ---------- | ----------------------------------------------------------------- |
| EVM     | `alloy-consensus`,`alloy-eips`                                | 2.5.0    | MIT OR Apache-2.0 | 已在产物里                              | 已在产物里 | 整组 85,Android 86.新增 40,另有 5 个已有 crate 的其他版本         |
| EVM     | `alloy-primitives`,`alloy-dyn-abi`(`eip712`),`alloy-json-abi` | 1.7.3    | MIT OR Apache-2.0 | 已在产物里                              | 已在产物里 | 含在上一行                                                        |
| EVM     | `alloy-rlp`                                                   | 0.3.17   | MIT OR Apache-2.0 | 已在产物里                              | 已在产物里 | 含在上一行                                                        |
| TRON    | `prost`                                                       | 0.14.4   | Apache-2.0        | 已进 `bitone-core`,纯 Rust,无新增 C     | 同左       | 整组 10,`Cargo.lock` 新增 2.`prost-derive`,`itertools` 只在编译期 |
| Bitcoin | `bitcoin`                                                     | 0.32.102 | CC0-1.0           | 已在产物里                              | 已在产物里 | 整组 12,新增 0                                                    |
| Bitcoin | `psbt-v2`                                                     | 0.3.0    | CC0-1.0           | GitHub 只读镜像没有 CI,只依赖 `bitcoin` | 同左       | 新增 1.`Cargo.lock` 里的 `miniscript` 是可选依赖,不编译           |
| Solana  | 无                                                            | 无       | 无                | 不涉及                                  | 不涉及     | 新增 0.遍历器和生成的表在 `bitone-core` 里                        |

- EVM 用 `default-features = false`,`features = ["std"]`,关掉 `kzg`,`k256`,`secp256k1`,`serde`.授权人恢复用现有的 secp256k1.
- `alloy-consensus`,`alloy-eips`,`alloy-primitives` 进入 `bitone-core` 后,`cargo tree -e normal` 的 crate 数在 wasm 上从 82 到 119,Android 从 85 到 122,新增 37 个,没有新的 C 依赖.
- `alloy-consensus` 要求 Rust 1.94.1,低于工具链的 1.95.0.
- `prost` 用 `default-features = false`,`features = ["derive", "std"]`.进入 `bitone-core` 后 `cargo tree -e normal` 的 crate 数在 wasm 上从 119 到 123,Android 从 122 到 126.不算过程宏时是 98 和 101,进产物的只新增 `prost`.`bytes`,`either` 已经由 alloy 带入.
- 5 个其他版本是 `sha3 0.11`,`sha2 0.10`,`keccak 0.2`,`generic-array 0.14.9`,`crypto-common 0.1.6`.`deny.toml` 的 `multiple-versions` 是警告级别.
- `alloy-dyn-abi` 用 `default-features = false`,`features = ["std", "eip712"]`.进入 `bitone-core` 后 `cargo tree -e normal` 的 crate 数在 wasm 上从 123 到 141,Android 从 126 到 142,没有新的 C 依赖.新增的过程宏 `alloy-sol-macro`,`syn-solidity` 等不进产物.
- `psbt-v2` 还没到 1.0,近期下载量约 5600.它属于 `rust-bitcoin` 组织,只给 PSBT v2 用,v0 继续走 `bitcoin`.用 `default-features = false`,`features = ["std"]`.

### 6.2 只在开发和 CI 使用

这些依赖不进 wasm 和 Android 的构建图.

| 用途           | crate 或工具                                                                                              | 版本                                 | 许可                         | 说明                                                               |
| -------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------ | ---------------------------- | ------------------------------------------------------------------ |
| 生成 TRON 类型 | `prost-build`,`protox`                                                                                    | 0.14.4,0.10.0                        | Apache-2.0,MIT OR Apache-2.0 | `protox` 是纯 Rust,不需要 `protoc`.`tools/protogen` 共 68 个 crate |
| Solana 对照    | `solana-vote-interface`,`solana-hash`,`solana-pubkey`,`solana-instruction`,`tools/crosscheck` 的 dev 依赖 | 7.3.0,4.7.0,4.4.0,4.0.0              | Apache-2.0                   | Vote 的 IDL 是手写的,要和 interface crate 比较                     |
| Solana 对照    | `solana-stake-interface`,`tools/crosscheck` 的 dev 依赖                                                   | 4.4.0,5.1.1                          | Apache-2.0                   | Stake 两种账户布局的字节来自这两个版本的构建函数                   |
| Solana 对照    | `@codama/dynamic-parsers`,`@codama/dynamic-codecs`,`codama`,RPC 的 `jsonParsed` 输出                      | 1.4.2,1.3.0,1.11.0                   | MIT                          | 不需要编译 Agave                                                   |
| EVM,TRON 对照  | `viem`,`tronweb`                                                                                          | 沿用 `apps/web` 锁定的版本或单独固定 | MIT                          | 运行时已用 alloy 和 prost,对照必须是另一套实现                     |
| Bitcoin 对照   | Bitcoin Core `decodepsbt`,`decoderawtransaction`                                                          | CI 固定版本                          | MIT                          | 已用于 regtest 验证                                                |
| 模糊测试       | `cargo-fuzz`                                                                                              | CI 固定版本和 nightly                | MIT OR Apache-2.0            | 只在 CI                                                            |

官方 interface crate 不进产物,原因是:

- 新增 84 个 crate,比另外三条链的总和还多.`solana-address`,`solana-instruction`,`solana-instruction-error`,`solana-pubkey`,`solana-serialize-utils` 各解析出两个版本.
- `spl-token-2022-interface` 在 wasm 上引入 `getrandom 0.2`(`js`),`js-sys`,`wasm-bindgen`,`curve25519-dalek 4`.
- 它们没有字节范围,`unpack` 丢弃尾部字节,`unpack_with_rest` 是 `pub(crate)`.System,Stake,Vote 还要额外启用 `wincode` 或 `bincode`.
- 官方 Codama IDL 已经是机器可读的格式定义,核对过:System 14,Token 28,Token-2022 100,Compute Budget 5,Address Lookup Table 5 都和 interface crate 的枚举一一对应.IDL 用到的类型节点只有 23 种.

### 6.3 Schema 来源

| 来源                                                          | 固定方式                                                                     | 许可                                                  |
| ------------------------------------------------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------- |
| java-tron `protocol/src/main/protos/core/`,在 `schemas/tron/` | tag `GreatVoyage-v4.8.2.4`,commit `4e8389f159d0782f1ebcd9dab2fd88388c0e6fe9` | 仓库 LGPL-3.0,`account_contract.proto` 文件头是 GPLv3 |
| `solana-program/*` 的 `idl.json`                              | commit 和 SHA-256                                                            | Apache-2.0                                            |
| Stake 的旧布局,在 `schemas/solana/stake-older.json`           | `solana-program/stake` 的 commit 和 SHA-256                                  | Apache-2.0                                            |
| ERC-7730 描述文件,在 `schemas/erc7730/`                       | `ethereum/clear-signing-erc7730-registry` 的 commit 和 SHA-256               | CC0-1.0                                               |

### 6.4 固定方法

1. `Cargo.toml` 写精确版本 `=x.y.z`,`Cargo.lock` 入库,升级是单独的改动,要重跑语料.
2. 新增依赖在 `deny.toml` 写明理由,`cargo deny` 检查许可,来源和重复版本.
3. Schema 文件放在 `schemas/`,`schemas/lock.toml` 记录仓库,commit,路径,SHA-256.生成器先校验哈希.TRON 的生成器是 `tools/protogen`,产物是 `decode/generated/` 和 `assets/tron-schema.json`,`--check` 在 CI 里比较产物.ERC-7730 的文件由核心库直接编译进去,`tools/protogen` 只校验它们的哈希.
4. 生成器和对照工具放在工作区之外,有自己的锁文件和工具链.
5. CI 定期比较上游 schema,有变化时开 PR,不自动合并.

### 6.5 未通过的项目

没有候选项被证实不能在 wasm32-unknown-unknown 或 Android 上构建.下面的项目在其他要求上没有通过:

| 项目                                           | 问题                                                  | 处理                           |
| ---------------------------------------------- | ----------------------------------------------------- | ------------------------------ |
| java-tron proto                                | LGPL-3.0 和 GPLv3 文件头都不在 `deny.toml` 的允许列表 | 保留,仓库是私有的              |
| `solana-config-interface` 2.0.1                | crates.io 上许可是 `non-standard`                     | 不使用,Config 走 IDL           |
| `solana-transaction-status` 4.4.0              | 要求 Rust 1.98.1,高于工具链                           | 不使用,用 RPC 的 `jsonParsed`  |
| `spl-associated-token-account-interface` 2.0.0 | 缺 IDL 里的 `createWithArgs`,发布于 2025-08           | IDL 为准,这一条没有 crate 对照 |
| `loader-v3` 的 `idl.json`                      | 内容为空                                              | 第 5.3 节的已知缺口            |

## 7. 应用层

### 7.1 信任来源

1. 描述文件只通过已验签的清单到达.清单用厂商 P-256 密钥签名,版本只增不减,见[安全设计 §2.9](04-security.md#29-内置清单更新).
2. 二维码只是传输层.`bitone-manifest` 也走二维码,它生效靠厂商签名.`bitone-request` 的载荷,`hint`,以及任何没有验签的内容里出现的描述文件,一律不信任,也不解析.
3. 描述文件来自公开注册表时,由厂商挑选并固定 commit.ERC-7730 明确要求假定注册表会被投毒,所以进清单的每个文件都要人工审查.

### 7.2 EVM 和 TRON

格式是 ERC-7730,schema 主版本 2(当前 2.1.0).ERC-7730 仍是草案.主版本不符的描述文件按无效处理.

| 规则   | 内容                                                                                                                                                                                                                                                                                                                                                                                               |
| ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 绑定   | 调用数据按链,目标地址,选择器匹配,选择器由函数签名算出.EIP-712 按链,`verifyingContract`,`primaryType`,类型串匹配                                                                                                                                                                                                                                                                                    |
| 解码   | 按描述里的 ABI 用 `alloy-dyn-abi` 解码.描述文件的函数重新编码必须等于 `data`,值也要在类型范围内,窄整数,定长字节的多余位不算相等.不相等按没有描述处理                                                                                                                                                                                                                                               |
| 类别   | 描述文件只决定字段名和展示.类别由策略按解码出的动作判定,不能降低.清单的描述文件给的类别是已识别,附带原生币时是无法识别,标准函数按规则表,种类名是 `fn:` 加规范签名                                                                                                                                                                                                                                  |
| 字段   | 每个 ABI 参数都进树,描述文件没写的参数显示 ABI 名字,所以描述文件不能隐藏参数.`path` 只认顶层参数,嵌套字段显示 ABI 名字                                                                                                                                                                                                                                                                             |
| 嵌套   | `calldata` 格式的参数按同一合约递归解码,最多 3 层,每层 32 条.超过时这一层按没有描述处理                                                                                                                                                                                                                                                                                                            |
| 内置   | `transfer`,`approve`,Permit 来自注册表的 `ercs/calldata-erc20-tokens.json`,`ercs/eip712-erc2612-permit.json`,其余标准函数在 `schemas/erc7730/bitone-calldata.json`,`bitone-eip712.json`,格式相同.内置描述不随清单改变                                                                                                                                                                              |
| 清单外 | 标准函数在任何合约上都解码显示,用户才看得到被授权方和额度,这是钓鱼的主要入口.类别是无法识别,告警写明合约不在清单,不一定遵守标准.无限额度的告警保留                                                                                                                                                                                                                                                 |
| 优先级 | 标准函数的选择器不看清单的描述文件,数据不规范时也一样,所以描述文件不能替换授权和转账的解读                                                                                                                                                                                                                                                                                                         |
| 不规范 | 标准函数的数据不规范时合约照样执行,Solidity 忽略尾部字节,旧版编译器屏蔽高位,缺的字节读成 0,钓鱼者可以故意这样构造.所以按这些编译器的读法取参数:地址取低 20 字节,整数取低位,定长字节取高位,布尔值非 0 即真,缺的字节是 0.方法,被授权方或收款方,金额都照常显示,对方在页面顶部,无限额度的告警保留.多余字节是 `opaque` 字段.类别是无法识别,告警写明数据不规范和合约会读到什么,可以签.EVM 和 TRC-20 相同 |
| 升级   | 描述绑定地址,不绑定代码.合约升级后含义可能改变,离线设备无法发现,所以描述文件给不出普通转账,普通转账只给清单内的代币                                                                                                                                                                                                                                                                                |

TRON 的 TRC-20 调用使用同一份 ABI 规则,地址按 TRON 的格式显示.

### 7.3 Solana

Solana 的第三方程序没有描述文件.程序 ID 不在第 5.3 节的表里时按第 7.4 节显示,类别是无法识别.

### 7.4 没有描述文件

| 链     | 显示                                                      |
| ------ | --------------------------------------------------------- |
| EVM    | 目标地址,选择器,`value`,`data` 整段一行十六进制           |
| TRON   | 同上,加 `call_token_value`,`token_id`                     |
| Solana | 程序 ID,账户列表和签名者,可写标记,`data` 的长度和十六进制 |

类别是无法识别,长按签名.不按固定偏移切分参数,第 2.4 节禁止手算偏移.唯一的例外是标准函数的不规范数据,见第 7.2 节.函数签名数据库不是可信来源,不用来猜测方法名.

### 7.5 清单格式

清单增加 `descriptors`,取代 `functions`.`descriptors` 是可选字段,没有它的清单包照常生效.每一项是 `network`,`contract`,带参数名的函数 `signature`,和 ERC-7730 `display.formats` 同构的 `intent`,`fields`.

清单本身是 JSON,描述文件进清单之前由厂商挑出用到的部分,不另外编码.内置的标准函数编译在核心库里,不在清单中,导入清单包不影响它们.内置清单没有大小限制,经二维码导入的清单包受 `MAX_MESSAGE_BYTES`(256 KiB)限制.

## 8. 验证

对照工具放在工作区之外的 `tools/crosscheck/`.普通 PR 的 CI 只读入库的语料快照,不访问网络.

### 8.1 官方向量和对照

| 链      | 官方向量                                                             | 独立对照                                                                              |
| ------- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| EVM     | `ethereum/tests` 的 `TransactionTests`.EIP-712 规范示例              | `viem`,类型 0 到 4,ABI 解码,EIP-712 摘要,EIP-7702 授权,`personal_sign`                |
| TRON    | java-tron 主网 `txID`                                                | `tronweb`,它的 `*_pb.cjs` 由同一份 proto 生成,缺 3 种合约.TIP-191,TIP-712,TRC-20 调用 |
| Solana  | `solana-program/*` 的 IDL 和 interface crate                         | interface crate,`@codama/dynamic-parsers`,RPC 的 `jsonParsed`                         |
| Bitcoin | BIP-143,BIP-174,BIP-341,BIP-370 向量,Bitcoin Core 的 `rpc_psbt.json` | Bitcoin Core `decodepsbt`                                                             |

比对到字段级:对照工具解出的每个字段都要和树里同名节点相等.`tools/crosscheck/` 里的名字映射表是数据文件.

Bitcoin 的官方向量在 `tools/crosscheck/vectors/bitcoin.json`,`vectors.lock` 记录哈希,文件里写明 `bitcoin/bips` 的 commit.BIP-174,370,371 的 PSBT 向量和 `bitcoin`,`psbt-v2` 直接解出的结果逐字段比较,BIP-143,341 的向量比较 sighash.

Solana 的向量在 `tools/crosscheck/vectors/solana.json`,由 `tools/crosscheck/codama/generate.mjs` 生成.运行时用自己的遍历器解码,所以字段的期望值来自 `@codama/dynamic-parsers`,指令的字节由 `@codama/dynamic-codecs` 从 IDL 编码.每条 IDL 指令都放进 legacy,v0,v1 三种消息,检查 C1,C2,C4,字段和账户名逐个相同,没有 `opaque` 节点,显示文字没有缺失.少一个必需账户时必须解码失败.Vote 的 IDL 是手写的,另外和 `solana-vote-interface` 比较.语料没有 Solana 样本之前,这些向量证明的是遍历器和 IDL 一致,不是 IDL 和程序一致.

Stake 的两种账户布局在 `tools/crosscheck/vectors/stake.json`,由 `solana-stake-interface` 4.4.0 和 5.1.1 的构建函数生成,账户名来自调用时传入的角色,不来自 `schemas/solana/`.`tools/crosscheck/src/stake_interface.rs` 检查文件和构建函数一致,`bitone-signer` 的测试检查账户行和第一屏.

EVM 的向量在 `tools/crosscheck/vectors/evm.json`,由 `tools/crosscheck/viem/generate.mjs` 生成,viem 的版本固定,名字映射表是 `tools/crosscheck/viem/names.json`.运行时用 alloy 解码,所以字段的期望值来自 viem.类型 0 到 4 用测试私钥由 viem 构建,`ethereum/tests` 的合法向量由 viem 解开,生成时核对官方的哈希和发送方.树里的字段,未签名编码,装回签名的字节,交易哈希,恢复出的发送方和授权人都要相同.官方的不合法向量去掉签名三项后必须解码失败.类型 3,4 没有官方向量进入快照.

TRON 的向量在 `tools/crosscheck/vectors/tron.json`,由 `tools/crosscheck/tronweb/generate.mjs` 生成,tronweb 的版本固定,样本是 `tools/crosscheck/tronweb/samples.json`.运行时用 prost 解码,所以字段的期望值来自 tronweb 的 `*_pb.cjs`,样本要填过 tronweb 认识的每个字段.41 个合约类型各一笔,另有备注,`Permission_id`,链上不使用的字段,未知字段,未知合约类型.树和向量逐字段双向比较,txID,装回签名的字节,恢复出的发送方都要相同,C1,C2,C4 都检查.tronweb 6.5.1 没有 `market_contract`,`shield_contract`,这 3 种合约用 `google-protobuf` 的 `BinaryWriter` 按样本里的编号写.`CustomContract`,`GetContract` 没有消息,只比外层.不合法的载荷必须解码失败.这份向量全部是构造的交易,主网 `txID` 等语料任务.

调用和消息的向量在 `tools/crosscheck/vectors/evm-messages.json`,`tron-messages.json`,由 `viem/messages.mjs`,`tronweb/messages.mjs` 生成,样本是 `viem/samples.json`,`tronweb/message-samples.json`,不访问网络.标准函数的每个参数按顺序和对照工具解出的值比较,带尾部字节的调用不能读出字段.EIP-712 比较域分隔符,消息哈希和签名原文,样本含规范的 Mail,Permit,Permit2,DAI 的 Permit,全部原子类型,嵌套数组,自引用和互相引用的类型,非规范顺序的域,没有 `chainId` 的域.TIP-191,TIP-712 另外用同一助记词比较 tronweb 的签名.

### 8.2 语料

每条链一份真实主网交易快照,放在 `tools/crosscheck/corpus/<chain>.jsonl`,`corpus.lock` 记录哈希.`.github/workflows/corpus.yml` 按周运行,取数后开 PR,不自动合并.只用公开 RPC,不访问 BitOne 的线上服务.

| 链      | 获取                                 | 自证                                                                       |
| ------- | ------------------------------------ | -------------------------------------------------------------------------- |
| EVM     | 已确认区块和原始交易                 | `keccak` 等于交易哈希,恢复出的签名者等于 RPC 的 `from`,去掉签名得到载荷    |
| TRON    | `wallet/getblockbynum`               | `sha256(raw_data)` 等于 `txID`,恢复出的地址等于 `owner_address` 或它的权限 |
| Solana  | `getBlock` 的 base64 和 `jsonParsed` | 每个 ed25519 签名都能在消息上验证通过                                      |
| Bitcoin | Esplora 的原始区块                   | `txid` 相符.PSBT v0,v2 由 Bitcoin Core 从原始交易和前序交易构造            |

- 抽样分两部分:最近已确认区块的随机样本,加 `corpus/seeds/<chain>.txt` 里针对稀有类型的交易.
- 第 5 节矩阵的每一行至少一个样本.确实没有的行写进 `corpus/absent.toml` 并注明原因,比如 TRON 的 3,20,32.
- CI 输出覆盖报告:每一行的样本数和比对结果.

每个样本检查 C1,C2,字段级对照,策略结果.另外做逐字节变异:翻转载荷的每个字节,结果必须是解码失败或某个叶子的值改变.这一项只在 CI 跑.

### 8.3 模糊测试

- 只在 CI 跑,`.github/workflows/fuzz.yml` 定时和手动触发.`Makefile` 没有模糊测试目标,不在开发机上运行.
- `cargo-fuzz` 用固定版本的 nightly,不改变仓库的 1.95.0 工具链.种子来自语料.
- 目标:`decode_evm`,`decode_typed_data`,`decode_tron`,`decode_solana`,`decode_psbt`,`decode_abi`,`walk_idl`.
- 性质:不崩溃,不超时.解码成功时 C1,C2 成立.解码成功时对照工具也必须接受,字段相同.
- 崩溃样本作为构件上传,修复后进回归测试.发布配置是 `panic = "abort"`,解码器不允许 panic.

## 9. 事实和假设

### 9.1 已核实

| 事实                                                                                           | 来源                                                                                                                         |
| ---------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `ContractType` 有 41 个值,`raw` 和 `Contract` 的字段如第 5.2 节                                | java-tron `Tron.proto`,tag 和 master 一致,`develop` 的枚举相同                                                               |
| 节点比较 `expiration` 和区块时间,不看 `timestamp`,上限 24 小时                                 | java-tron `Manager.validateCommon`,`Constant.java`                                                                           |
| 节点用 protobuf-java 3.25.8,未知字段写在已知字段之后,`Any.value` 是字节,不重新序列化           | java-tron `protocol/build.gradle`,protobuf-java 的 `UnknownFieldSet`                                                         |
| 20,32 没有消息定义和执行器,3 有消息没有执行器,51 有执行器                                      | java-tron `protos/core/contract/`,`actuator/` 目录                                                                           |
| 备注按 `getMemoFee` 扣费,合约数必须为 1,哈希取 `getRawData().toByteArray()`                    | java-tron `Manager.java`,`TransactionCapsule.java`                                                                           |
| 协议文件的许可                                                                                 | GitHub API,`account_contract.proto` 文件头                                                                                   |
| SIMD-0385 状态 `Activated`,v1 的格式和限制,feature 公钥在 Agave 里                             | `solana-foundation/solana-improvement-documents`,`anza-xyz/agave` 的 `feature-set`                                           |
| 链下消息的三种格式,v1 的掩码和签名位置                                                         | `anza-xyz/solana-sdk` 的 `offchain-message`,`message`,`transaction`,`anza-xyz/kit` 的 `packages/offchain-messages`,SIMD-0385 |
| Vote 的 20 条指令和 12 条管理指令的字段,账户                                                   | `solana-vote-interface` 7.3.0 的源码                                                                                         |
| Solana 指令集合和 IDL 指令数                                                                   | `solana-program/*` 的 `idl.json`,对应 interface crate 的源码                                                                 |
| EIP-7702 是 Final,EIP-8141 是 Draft,类型 0x06.`TxEip8141` 不是 `TxEnvelope` 的成员             | `ethereum/EIPs`,`alloy-consensus` 2.5.0 源码                                                                                 |
| `TxLegacy::decode`,`TokenInstruction::unpack`,`prost` 的行为                                   | 对应 crate 的源码                                                                                                            |
| crate 版本,许可,Rust 版本要求                                                                  | crates.io API                                                                                                                |
| 依赖量                                                                                         | `cargo tree`,工具链 1.95.0,只解析依赖,没有编译                                                                               |
| 上游 wasm 构建情况                                                                             | `alloy-rs/alloy`,`alloy-rs/core`,`anza-xyz/solana-sdk`,`solana-program/token-2022`,`tokio-rs/prost` 的 CI 文件               |
| ERC-7730 是 Draft,schema 版本 2.1.0,注册表已迁到 `ethereum/clear-signing-erc7730-registry`,CC0 | `ethereum/ERCs`,GitHub API                                                                                                   |
| TIP-191 的前缀和长度,TIP-712 的地址,`trcToken`,`chainId`                                       | `tronprotocol/tips`,tronweb 6.5.1 的 `utils/message.js`,`utils/typedData.js`                                                 |
| TRON 的 `eth_chainId`,Shasta 0x94a9059e,Nile 0xcd8690dc                                        | TronGrid 的 JSON-RPC,TIP-712 只列出主网和 Nile                                                                               |
| `TypedData` 的域哈希不看声明的类型,`Resolver::resolve` 拒绝自引用类型                          | `alloy-dyn-abi` 1.7.3 源码                                                                                                   |
| UniFFI 0.32.0 起支持递归记录                                                                   | `mozilla/uniffi-rs` 的 `CHANGELOG.md`                                                                                        |
| PSBT 键的集合                                                                                  | BIP-174,370,371,373,375,353,127 和 `psbt-v2` 的 `consts.rs`.BIP-431,433 目前是 Draft                                         |

### 9.2 假设,未验证

1. **构建**:`alloy-consensus`,`alloy-eips`,`alloy-primitives`,`alloy-rlp`,`psbt-v2` 已在 `bitone-core` 里构建.其余候选依赖的 wasm 和 Android 结论来自上游 CI 和依赖图,图里没有新增的 C 依赖.`prost` 已进入 `bitone-core`,开发机上只构建了宿主机,wasm 和 Android 由 CI 构建.`solana-message` 在 wasm 上没有直接的上游证据.
2. **体积**:没有测量 WASM 和 `.so` 的体积增长.
3. **主网日期**:v1 在 2026-09-15 前后激活来自二手报道,没有核对主网的 feature 账户.`Activated` 状态来自 SIMD 仓库.
4. **规范编码**:假设主网 TRON 交易都是规范字段顺序,`auths`,`scripts`,`provider`,`ContractName`,`ref_block_num` 没有合法用途.语料会验证,不成立时调整规则.
5. **类别**:矩阵里的风险类别是本标准的判断,不来自官方.
6. **IDL 准确性**:假设 IDL 和程序行为一致.`batch` 的内部指令在 IDL 里是字节数组,递归解码是遍历器的规则,不是 IDL 给出的.对照测试会验证.
7. **其他链**:BSC,Polygon 对各交易类型的支持没有逐一核实,规则按 Chain ID 一视同仁.
8. **语料**:公开 RPC 的可用性和限流按现有实测估计,CI 上没有验证过.
9. **许可**:第 6 节的许可判断不是法律意见.
