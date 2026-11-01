# 交易解码标准

本文规定 BitOne 怎样解码待签名数据.Bitcoin,EVM,TRON,Solana 都按本文实现,手机和网页端共用核心库的同一份结果.

协议能承载什么,需求就是什么,和网页端当前发送什么无关.用户确认之前,必须能看到这笔交易的全部字段.

版本和上游状态核实于 2026-11-01.事实来源和假设分开列在第 10 节.

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

| 情况                 | 例子                                                                     |
| -------------------- | ------------------------------------------------------------------------ |
| 请求不属于本钱包     | 主密钥指纹不符                                                           |
| 请求自相矛盾         | 派生路径和网络不符,地址和路径不符,Chain ID 和网络不符,前序交易和输入不符 |
| 已知格式的字节不合规 | 非规范编码,多余字节,索引越界,这些内容链也不接受,不同解析器的结果可能不同 |
| 签名无法正确完成     | 解锁后仍推导不出对应私钥,`SIGHASH_SINGLE` 没有对应输出,输入脚本不认识    |
| 超过资源上限         | 整包,帧数和嵌套深度超过解析上限                                          |

其余内容都可以签:

- 未来的交易类型,合约类型,指令和程序:能解码的外层字段照常解码,其余作为 `opaque` 节点显示原始数据.
- 已知的签名域先按真实含义解码,不算未知格式:`0x19` 是 EIP-191 和 EIP-712,`0x05` 是 EIP-7702 授权,Solana 的 `0xff` 开头是链下消息.
- 其余载荷都不认识时,树只有一个 `opaque` 根节点.未知的 EIP-2718 类型按 `keccak256(payload)` 计算签名原文.
- 未知网络:比如本地调试链或以后的新链.页面显示请求里的网络名,类别是无法识别,只要不能确认是测试网就显示主网提示.路径和网络的矛盾只对本机认识的网络检查,EVM 请求的路径例外,见下面的派生路径.Bitcoin 的 `paths` 是空的,所以看 `chain` 字段,没有时看 PSBT 魔数.
- EVM 请求的网络名本机不认识,Chain ID 本机认识时,以 Chain ID 为准,请求里的网络名不能改变显示,告警写明这一点.两边都不认识时显示请求里的网络名和 Chain ID.
- 路径不能单独判定链:EVM 路径不区分主网和测试网,coin type `1'` 可能是 TRON,Solana 或 Bitcoin 测试网.`SignRequest` 增加可选的链字段,没有时才按路径和载荷推断.
- 危险但合法的行为:裸哈希签名,消息签名,账户委托,不绑定输出的 sighash,没有 Chain ID 的交易,都是无法识别,告警要说清后果.裸哈希和整段未知载荷在长按之后还要二次确认.`0x19 0x01` 摘要和本机不认识的 EIP-191 版本看不到内容,同样要二次确认.
- 内置清单缺失或损坏不影响签名,只影响名称和图标.
- 派生路径不设白名单,任意路径都可以花费和导入,页面显示路径并告警.例外是 EVM 请求落在本钱包其他链主网账户的用途和 coin type 上,比如 `m/84'/0'`,这是矛盾,网络名本机不认识时也拒绝.这些私钥上有真实资产,裸哈希能签下它们的交易,改个网络名不能绕过.Bitcoin 找零的路径和索引也不设上限,本机能重新算出地址就显示为本钱包地址,路径不常见或索引跳得太远时告警并要求长按.声明是本钱包却算不出同一地址时拒绝.保存的公钥算不出的硬化路径解锁后用私钥核对.
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

| 载荷     | 签名原文                  | 结构遍历        | 字段含义来自                   | C2                                                   |
| -------- | ------------------------- | --------------- | ------------------------------ | ---------------------------------------------------- |
| EVM 交易 | 类型字节加 RLP            | RLP 项          | `alloy-consensus` 类型         | 官方编码.旧式交易另查 9 项形式,末两项必须为 0        |
| EIP-712  | 由类型算出的哈希          | JSON 成员       | `alloy-dyn-abi` 的 `TypedData` | 重新计算 `hashStruct`                                |
| TRON     | `raw_data` 字节           | protobuf 线格式 | 官方 proto 生成的类型          | `prost` 编码                                         |
| Solana   | 消息字节                  | 消息布局        | 官方 Codama IDL                | 按 IDL 重新编码                                      |
| Bitcoin  | 未签名交易加 sighash 原文 | PSBT 键值       | `bitcoin`,`psbt-v2` 类型       | 交易的共识编码.PSBT 映射内的键序不固定,这一层只用 C1 |

- PSBT v2 没有交易字节,C2 比较遍历器读到的字段和官方解码结果拼成的交易.遍历器和官方解码器读到的条目数也要相同.
- EIP-712 的载荷是 JSON,字节本身不进签名原文.C1 改为:每个 JSON 成员都被类型定义消费,重复键和多余成员都失败.`serde_json` 对重复键保留最后一个,所以要用能发现重复的解析方式.
- TRON 节点用 `getRawData().toByteArray()` 重新序列化后取哈希.字段乱序或显式写默认值的载荷,哈希和节点算出的不同,所以 C2 不比节点更严.
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

请求信封的检查顺序不变,见[系统设计 §6](03-system.md).网络和派生路径不在载荷里,作为 `derived` 节点进树.

1. 策略只读树,拿不到原始载荷.`crates/bitone-core/tests/architecture.rs` 检查 `policy` 只依赖模型,`decode` 不依赖 `policy`,`manifest`,`review`.
2. 规则是白名单.规则表按节点种类给类别,种类名来自 schema.没有规则的节点是无法识别,普通转账只能由规则显式给出.
3. 没有动作的交易是无法识别.
4. 一笔交易取全部动作里最高的类别.告警只升级确认方式,不降低类别.
5. 规则表是核心库内置的数据文件.清单和描述文件都不能修改规则,描述文件也不能降低类别.
6. CI 检查三方一致:官方枚举,第 5 节矩阵对应的规则表,语料样本.重新生成 schema 后出现新类型时 CI 失败,补上规则才能合并.

| 顺序 | 类别     | 含义                                              | 确认                                                                        |
| ---- | -------- | ------------------------------------------------- | --------------------------------------------------------------------------- |
| 6    | 禁止     | 第 1.1 节列出的拒绝情况                           | 拒绝                                                                        |
| 5    | 无法识别 | 格式或含义不认识,没有规则,没有描述,或规则标为危险 | 长按,最强告警.规则的 `second` 为真和根节点是 `opaque` 时,长按后还要二次确认 |
| 4    | 权限变更 | 账户控制权改变                                    | 长按                                                                        |
| 3    | 授权     | 第三方可以动用资产                                | 长按                                                                        |
| 2    | 已识别   | 含义完整,不是普通转账                             | 点按,有告警时长按                                                           |
| 1    | 普通转账 | 白名单内的原生币和清单内代币转账                  | 点按,有告警时长按                                                           |

无法识别排在权限变更之上,一笔同时含授权和未知调用的交易按无法识别告警.

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

record Summary
  title: string
  amount: ReviewAmount?
  counterparty_label: string?
  counterparty: string?
  field_ids: list<u32>

record ReviewResult
  risk: RiskLevel
  confirm: ConfirmMode
  network: string
  mainnet: bool
  summary: Summary
  alerts: list<string>
  fields: list<FieldRow>
  field_count: u32
  review_digest: bytes
  refusal: string?
  fingerprint_mismatch: bool
  request_fingerprint: string?
```

### 4.2 渲染规则

1. 手机只有一个通用列表,不按链,不按字段名分支.分组只看 `parent` 和 `depth`,`INPUT_LABELS`,`PARTY_LABELS`,`FIRST_SCREEN_LABELS` 这类按标签分组的逻辑不再存在.
2. `value` 由核心库按 `kind` 格式化,完整不截断.手机不做格式化.
3. `label` 来自核心库的数据文件,键是 `name`.CI 检查没有缺失,运行时缺失就显示 `name`.
4. 摘要在列表上方,不能代替列表.摘要的每个值都引用 `field_ids`,所以摘要不可能显示树里没有的内容.
5. `signed` 行不可折叠,`context` 行可以折叠.
6. 列表滚到底之前,确认控件不可用.
7. `field_count` 等于节点数.`review_digest` 是全部行,风险和确认方式的 SHA-256.手机把它,已渲染行数和二次确认的结果传回 `sign_request`,核心库重新解码算出相同结果才签名,要二次确认而没有确认时拒绝.

### 4.3 需要重新生成

| 产物        | 命令                | 随之修改                                                                                                         |
| ----------- | ------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Kotlin 绑定 | `make android-core` | `UniffiWalletCore.kt`,`platform/Review.kt`,`ReviewScreen.kt`,`reviewStyle`,测试里的 `Fakes.kt`,`SignFlowTest.kt` |
| `jniLibs`   | 同上                | 无                                                                                                               |
| WASM 和类型 | `make wasm`         | `apps/web/src/core/index.ts`,`chains/transfer.ts`,`pages/Sign.tsx`                                               |

绑定和 `jniLibs` 都被 Git 忽略,由 CI 生成.Kotlin 的 `Risk` 枚举和 `risk >= Risk.Approval` 这类比较要随第 3 节的新顺序一起改.

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

- **按调用判定**:`to` 有值且 `data` 为空是普通转账.`to` 有值且 `data` 非空按第 7 节.`to` 为空是创建合约,无法识别.第 7 节的描述文件接入之前,合约调用的类别来自 `tx/abi.rs`,`data` 是 `bytes` 节点,调用方法和合约写在这一行的备注里.
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

| 消息                                                            | 解码                                     | 可签 | 类别                    |
| --------------------------------------------------------------- | ---------------------------------------- | ---- | ----------------------- |
| EIP-712 Permit,Permit2,类型串逐字相符                           | `TypedData`,全部字段                     | 是   | 授权                    |
| EIP-712 任意消息,有描述文件                                     | 同上,按描述展示                          | 是   | 不低于已识别,按内部动作 |
| EIP-712 任意消息,没有描述文件                                   | 同上,`domain`,`types`,`message` 全部显示 | 否   | 无法识别                |
| EIP-712 域缺 `chainId` 或 `verifyingContract`,或 `chainId` 不符 | 同上                                     | 否   | 无法识别                |
| `personal_sign`(EIP-191 版本 0x45)                              | 文本或字节                               | 是   | 无法识别                |
| EIP-191 版本 0x00                                               | 验证者地址,数据                          | 是   | 无法识别                |
| EIP-191 其他版本                                                | 版本,`opaque` 数据                       | 是   | 无法识别,二次确认       |
| `0x19 0x01` 加域分隔符和消息哈希                                | `domain_separator`,`hash_struct`         | 是   | 无法识别,二次确认       |
| `eth_sign`                                                      | 32 字节哈希,没有内容可解码               | 是   | 无法识别,二次确认       |
| EIP-7702 授权元组(前缀 0x05)                                    | `chain_id`,`address`,`nonce`             | 是   | 无法识别                |

`SignRequest.kind`:0 是交易和按载荷分辨的签名域,1 是 EIP-712 的 JSON,2 是 32 字节的裸哈希,3 是 `personal_sign` 的消息.其他取值不知道怎么算签名原文,拒绝.kind 0 的 `0x19`,`0x05` 载荷的签名原文是 `keccak256(payload)`,不符合对应格式时拒绝,不走未知类型的路径.

### 5.2 TRON

依据是 java-tron 的官方 proto,固定在 tag `GreatVoyage-v4.8.2.4`.全部消息由生成代码解码,每个字段都进树.

| 编号 | `Transaction.raw` 字段 | 规则                                                                                 |
| ---- | ---------------------- | ------------------------------------------------------------------------------------ |
| 1    | `ref_block_bytes`      | 2 字节                                                                               |
| 3    | `ref_block_num`        | 解码并显示,非 0 是无法识别                                                           |
| 4    | `ref_block_hash`       | 8 字节                                                                               |
| 8    | `expiration`           | 大于 `timestamp`,差值不超过 24 小时                                                  |
| 9    | `auths`                | 解码并显示,非空是无法识别                                                            |
| 10   | `data`                 | 备注.合法 UTF-8 是 `text`,否则是 `bytes`.非空时节点按 `getMemoFee` 额外扣费,所以告警 |
| 11   | `contract`             | 恰好 1 个,节点也这样要求                                                             |
| 12   | `scripts`              | 解码并显示,非空是无法识别                                                            |
| 14   | `timestamp`            | 时间                                                                                 |
| 18   | `fee_limit`            | 金额,超阈值告警                                                                      |

| 编号 | `Contract` 字段 | 规则                                                                           |
| ---- | --------------- | ------------------------------------------------------------------------------ |
| 1    | `type`          | 枚举.未知编号是未来的合约类型,`parameter` 作为 `opaque`,长按后二次确认          |
| 2    | `parameter`     | `type_url` 必须是 `type.googleapis.com/protocol.` 加和 `type` 相符的消息名     |
| 3    | `provider`      | 解码并显示,非空是无法识别                                                      |
| 4    | `ContractName`  | 解码并显示,非空是无法识别                                                      |
| 5    | `Permission_id` | 解码并显示,非 0 时告警.`owner_address` 必须是本账户,替其他账户多签是单独的工作 |

- 发起方字段按名字取 `owner_address`,和 java-tron 的 `getOwner` 一致,不再维护字段编号表.
- `int32`,`int64` 字段出现负数是无法识别.
- 编号 20,32,51 是原表缺少的类型.

| 编号 | 合约类型                        | 类别             | 说明                                                                |
| ---- | ------------------------------- | ---------------- | ------------------------------------------------------------------- |
| 0    | AccountCreateContract           | 已识别           |                                                                     |
| 1    | TransferContract                | 普通转账         |                                                                     |
| 2    | TransferAssetContract           | 普通转账或已识别 | 清单内的 TRC-10 是普通转账.清单外告警,金额显示原始整数              |
| 3    | VoteAssetContract               | 无法识别         | 有消息定义,没有执行器                                               |
| 4    | VoteWitnessContract             | 已识别           | 列出全部投票对象                                                    |
| 5    | WitnessCreateContract           | 已识别,告警      | 扣账户升级费                                                        |
| 6    | AssetIssueContract              | 已识别,告警      | 扣发行费                                                            |
| 8    | WitnessUpdateContract           | 已识别           |                                                                     |
| 9    | ParticipateAssetIssueContract   | 已识别,告警      | 用 TRX 换 TRC-10                                                    |
| 10   | AccountUpdateContract           | 已识别           |                                                                     |
| 11   | FreezeBalanceContract           | 已识别           | `receiver_address` 不是本账户时告警                                 |
| 12   | UnfreezeBalanceContract         | 已识别           | 同上                                                                |
| 13   | WithdrawBalanceContract         | 已识别           |                                                                     |
| 14   | UnfreezeAssetContract           | 已识别           |                                                                     |
| 15   | UpdateAssetContract             | 已识别           |                                                                     |
| 16   | ProposalCreateContract          | 已识别           | `parameters` 是 map,键序不规范时 C2 失败                            |
| 17   | ProposalApproveContract         | 已识别           |                                                                     |
| 18   | ProposalDeleteContract          | 已识别           |                                                                     |
| 19   | SetAccountIdContract            | 已识别           |                                                                     |
| 20   | CustomContract                  | 无法识别         | 只有枚举值,没有消息和执行器                                         |
| 30   | CreateSmartContract             | 无法识别         | `bytecode` 是应用层.其余字段,包括 `call_token_value`,全部解码       |
| 31   | TriggerSmartContract            | 按第 7 节        | `data` 是应用层.`call_value`,`call_token_value`,`token_id` 全部显示 |
| 32   | GetContract                     | 无法识别         | 只有枚举值,没有消息和执行器                                         |
| 33   | UpdateSettingContract           | 已识别           |                                                                     |
| 41   | ExchangeCreateContract          | 已识别,告警      | 资产转入交易对                                                      |
| 42   | ExchangeInjectContract          | 已识别,告警      | 同上                                                                |
| 43   | ExchangeWithdrawContract        | 已识别           |                                                                     |
| 44   | ExchangeTransactionContract     | 已识别,告警      | 显示 `quant`,`expected`                                             |
| 45   | UpdateEnergyLimitContract       | 已识别           |                                                                     |
| 46   | AccountPermissionUpdateContract | 权限变更         | 全部 `Permission` 和 `Key` 展开,`operations` 按合约类型编号解码位图 |
| 48   | ClearABIContract                | 已识别           |                                                                     |
| 49   | UpdateBrokerageContract         | 已识别           |                                                                     |
| 51   | ShieldedTransferContract        | 无法识别         | 冷钱包没有屏蔽密钥,`transparent_from_address` 可以为空              |
| 52   | MarketSellAssetContract         | 已识别,告警      | 挂单卖出资产                                                        |
| 53   | MarketCancelOrderContract       | 已识别           |                                                                     |
| 54   | FreezeBalanceV2Contract         | 已识别           |                                                                     |
| 55   | UnfreezeBalanceV2Contract       | 已识别           |                                                                     |
| 56   | WithdrawExpireUnfreezeContract  | 已识别           |                                                                     |
| 57   | DelegateResourceContract        | 授权             | 显示 `receiver_address`,`lock`,`lock_period`                        |
| 58   | UnDelegateResourceContract      | 已识别           |                                                                     |
| 59   | CancelAllUnfreezeV2Contract     | 已识别           |                                                                     |

### 5.3 Solana

| 消息     | 首字节       | 字段                                                                          | 规则                                                                                                                                                                                              |
| -------- | ------------ | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| legacy   | 最高位为 0   | 头部,账户表,`recent_blockhash`,指令                                           | 账户表不重复,索引不越界,和节点的 sanitize 一致                                                                                                                                                    |
| v0       | 0x80         | legacy 的字段加 `address_table_lookups`                                       | 同上                                                                                                                                                                                              |
| v1       | 0x81         | 头部,`TransactionConfigMask`,`lifetime_specifier`,地址,配置值,指令头,指令数据 | 未知掩码位之后的内容宽度未知,作为 `opaque`,长按后二次确认.没有查找表.上限 4096 字节,12 个签名,64 个账户,64 条指令,超过时告警,这些数字以后可能放宽.ComputeBudget 指令不影响配置,手续费只按配置值算 |
| 未知     | 0x82 到 0xfe | 无                                                                            | 未知格式,整段 `opaque`,长按后二次确认,签名原文是载荷本身                                                                                                                                          |
| 链下消息 | 0xff         | `\xffsolana offchain` 头部,版本和消息字段                                     | 按消息解码,字段以官方格式为准.头部完整但后面不合规时拒绝.没有这段头部时是未知格式                                                                                                                 |

- **手续费支付者**:必须是本账户,否则无法识别.
- **查找表**:表地址和索引全部解码,表里的地址是链上状态.账户位来自查找表时,节点是表地址加索引,不猜测地址.消息含查找表时是无法识别.
- **账户**:每个账户节点带 IDL 里的账户名,签名者和可写标记.账户数少于 IDL 必需账户时解码失败,程序本身不会执行.多出的账户程序通常忽略,作为 `opaque` 节点显示,类别是无法识别.
- **尾部字节**:程序本身会忽略尾部字节,链接受这种交易,所以不拒绝.它们作为 `opaque` 节点显示,类别是无法识别,C1 照常成立.
- **第三方程序**:程序 ID 不在下表时,按第 7 节处理.
- **Token-2022**:mint 的扩展是链上状态,转账时的费用和 hook 看不到.只有清单声明过扩展的 mint 才能是普通转账.

解码方式都是通用遍历器读官方 Codama IDL.指令名取自 IDL.

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

| 程序                 | 类别        | 指令                                                                                                                                                                                                                                                                                                           |
| -------------------- | ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| System               | 普通转账    | `transferSol`                                                                                                                                                                                                                                                                                                  |
| System               | 权限变更    | `assign`,`assignWithSeed`,`authorizeNonceAccount`                                                                                                                                                                                                                                                              |
| System               | 已识别,告警 | `transferSolWithSeed`,`withdrawNonceAccount`.`advanceNonceAccount` 不在第一条,或 nonce 账户不属于本钱包时也在这里                                                                                                                                                                                              |
| System               | 已识别      | `createAccount`,`createAccountWithSeed`,`createAccountAllowPrefund`,`allocate`,`allocateWithSeed`,`initializeNonceAccount`,`upgradeNonceAccount`,`advanceNonceAccount`                                                                                                                                         |
| SPL Token            | 普通转账    | `transferChecked`,mint 在清单内,小数位一致,账户恰好 4 个                                                                                                                                                                                                                                                       |
| SPL Token            | 授权        | `approve`,`approveChecked`                                                                                                                                                                                                                                                                                     |
| SPL Token            | 权限变更    | `setAuthority`                                                                                                                                                                                                                                                                                                 |
| SPL Token            | 已识别,告警 | `transfer`,清单外的 `transferChecked`,`burn`,`burnChecked`,`closeAccount`,`unwrapLamports`,`withdrawExcessLamports`                                                                                                                                                                                            |
| SPL Token            | 已识别      | `initializeMint`,`initializeMint2`,`initializeAccount`,`initializeAccount2`,`initializeAccount3`,`initializeMultisig`,`initializeMultisig2`,`revoke`,`mintTo`,`mintToChecked`,`freezeAccount`,`thawAccount`,`syncNative`,`getAccountDataSize`,`initializeImmutableOwner`,`amountToUiAmount`,`uiAmountToAmount` |
| SPL Token            | 取内部最高  | `batch`.内部每条指令按同一 IDL 解码.嵌套 `batch` 解码失败                                                                                                                                                                                                                                                      |
| Associated Token     | 已识别      | `create`,`createIdempotent`,`createWithArgs`.地址必须由所有者,mint,程序推导得出                                                                                                                                                                                                                                |
| Associated Token     | 已识别,告警 | `recoverNested`                                                                                                                                                                                                                                                                                                |
| Compute Budget       | 已识别      | `requestHeapFrame`,`setComputeUnitLimit`,`setComputeUnitPrice`,`setLoadedAccountsDataSizeLimit`.不算动作,进入手续费节点                                                                                                                                                                                        |
| Compute Budget       | 无法识别    | `requestUnits`,已废弃                                                                                                                                                                                                                                                                                          |
| Memo                 | 已识别      | `addMemo`.非法 UTF-8 解码失败                                                                                                                                                                                                                                                                                  |
| Stake                | 权限变更    | `authorize`,`authorizeChecked`,`authorizeWithSeed`,`authorizeCheckedWithSeed`,`setLockup`,`setLockupChecked`                                                                                                                                                                                                   |
| Stake                | 已识别,告警 | `withdraw`                                                                                                                                                                                                                                                                                                     |
| Stake                | 已识别      | `initialize`,`initializeChecked`,`delegateStake`,`split`,`deactivate`,`merge`,`getMinimumDelegation`,`deactivateDelinquent`,`moveStake`,`moveLamports`                                                                                                                                                         |
| Stake                | 无法识别    | `redelegate`,已废弃                                                                                                                                                                                                                                                                                            |
| Vote                 | 权限变更    | `authorize`,`authorizeChecked`,`authorizeWithSeed`,`authorizeCheckedWithSeed`,`updateValidatorIdentity`                                                                                                                                                                                                        |
| Vote                 | 已识别,告警 | `withdraw`                                                                                                                                                                                                                                                                                                     |
| Vote                 | 已识别      | `initializeAccount`,`initializeAccountV2`,`updateCommission`,`updateCommissionCollector`,`updateCommissionBps`,`depositDelegatorRewards`                                                                                                                                                                       |
| Vote                 | 无法识别    | `vote`,`voteSwitch`,`updateVoteState`,`updateVoteStateSwitch`,`compactUpdateVoteState`,`compactUpdateVoteStateSwitch`,`towerSync`,`towerSyncSwitch`                                                                                                                                                            |
| Address Lookup Table | 已识别      | `createLookupTable`,`extendLookupTable`,`deactivateLookupTable`,`closeLookupTable`                                                                                                                                                                                                                             |
| Address Lookup Table | 已识别,告警 | `freezeLookupTable`,不可逆                                                                                                                                                                                                                                                                                     |

**Vote**:官方没有 IDL,管理类指令写成仓库内的 IDL 数据文件,在 CI 里和 `solana-vote-interface` 比对.8 条共识投票指令用了 Codama 表达不了的变长编码,不进 IDL,命中时按 `opaque` 显示,类别是无法识别.这是本标准已知的缺口.

Token-2022 的 0 到 25 号,38,45,255 号和 SPL Token 同名指令规则相同,`initializeMintCloseAuthority` 是已识别.扩展指令:

| 前缀   | 扩展                    | 类别                 | 指令                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------ | ----------------------- | -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 26     | TransferFee             | 已识别               | `initializeTransferFeeConfig`,`withdrawWithheldTokensFromMint`,`withdrawWithheldTokensFromAccounts`,`harvestWithheldTokensToMint`,`setTransferFee`                                                                                                                                                                                                                                                                                                                                                  |
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
- `psbt-v2` 0.3.0 和 BIP-370 有三处不符,PSBT 进入它之前处理.`INPUT_COUNT` 排在 `OUTPUT_COUNT` 之后时它报重复键,所以键先按编号排序.金额 0 和空脚本它按缺失处理,所以换成占位值,字段的值以遍历器为准.`SEQUENCE` 缺失时它取 0,BIP-370 规定是 final,所以交易由遍历器拼出.

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

`decode` 不读密钥,树里的派生路径只是声明.`tx/bitcoin.rs` 重新派生后把本账户事实交给 `policy::assess_bitcoin`,它算出命中了哪些种类,类别和告警都来自 `assets/rules.json`.`version`,`lock_time`,`sequence` 的含义是规则表里的 `note`.

## 6. 依赖

### 6.1 运行时依赖

列的含义:依赖量来自 `cargo tree -e normal`,是该组在目标平台上的 crate 总数,"新增"是当前 `Cargo.lock` 没有的 crate 数.这些数字没有编译过,见第 10 节.

| 链      | crate                                        | 版本     | 许可              | wasm32-unknown-unknown                  | Android           | 依赖量                                                        |
| ------- | -------------------------------------------- | -------- | ----------------- | --------------------------------------- | ----------------- | ------------------------------------------------------------- |
| EVM     | `alloy-consensus`,`alloy-eips`               | 2.5.0    | MIT OR Apache-2.0 | 已在产物里                              | 已在产物里        | 整组 85,Android 86.新增 40,另有 5 个已有 crate 的其他版本     |
| EVM     | `alloy-primitives`,`alloy-dyn-abi`(`eip712`) | 1.7.3    | MIT OR Apache-2.0 | `alloy-primitives` 已在产物里           | 同左              | 含在上一行.`alloy-dyn-abi` 随第 6 步进入 `bitone-core`        |
| EVM     | `alloy-rlp`                                  | 0.3.17   | MIT OR Apache-2.0 | 已在产物里                              | 已在产物里        | 含在上一行                                                    |
| TRON    | `prost`                                      | 0.14.4   | Apache-2.0        | 上游只有 no-std 检查,纯 Rust            | 上游未测,无新增 C | 整组 10,新增 4.`prost-derive`,`itertools`,`either` 只在编译期 |
| Bitcoin | `bitcoin`                                    | 0.32.102 | CC0-1.0           | 已在产物里                              | 已在产物里        | 整组 12,新增 0                                                |
| Bitcoin | `psbt-v2`                                    | 0.3.0    | CC0-1.0           | GitHub 只读镜像没有 CI,只依赖 `bitcoin` | 同左              | 新增 1.`Cargo.lock` 里的 `miniscript` 是可选依赖,不编译       |
| Solana  | 无                                           | 无       | 无                | 不涉及                                  | 不涉及            | 新增 0.遍历器和生成的表在 `bitone-core` 里                    |

- EVM 用 `default-features = false`,`features = ["std"]`,关掉 `kzg`,`k256`,`secp256k1`,`serde`.授权人恢复用现有的 secp256k1.
- `alloy-consensus`,`alloy-eips`,`alloy-primitives` 进入 `bitone-core` 后,`cargo tree -e normal` 的 crate 数在 wasm 上从 82 到 119,Android 从 85 到 122,新增 37 个,没有新的 C 依赖.
- `alloy-consensus` 要求 Rust 1.94.1,低于工具链的 1.95.0.
- 5 个其他版本是 `sha3 0.11`,`sha2 0.10`,`keccak 0.2`,`generic-array 0.14.9`,`crypto-common 0.1.6`.`deny.toml` 的 `multiple-versions` 是警告级别.
- `psbt-v2` 还没到 1.0,近期下载量约 5600.它属于 `rust-bitcoin` 组织,只给 PSBT v2 用,v0 继续走 `bitcoin`.用 `default-features = false`,`features = ["std"]`.

### 6.2 只在开发和 CI 使用

这些依赖不进 wasm 和 Android 的构建图.

| 用途           | crate 或工具                                                                                                                                                                                                                                                                         | 版本                                                        | 许可                         | 说明                                                  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------- | ---------------------------- | ----------------------------------------------------- |
| 生成 TRON 类型 | `prost-build`,`protox`                                                                                                                                                                                                                                                               | 0.14.4,0.10.0                                               | Apache-2.0,MIT OR Apache-2.0 | `protox` 是纯 Rust,不需要 `protoc`.宿主机 49 个 crate |
| Solana 对照    | `solana-message`,`solana-system-interface`,`solana-compute-budget-interface`,`solana-address-lookup-table-interface`,`solana-stake-interface`,`solana-vote-interface`,`spl-token-interface`,`spl-token-2022-interface`,`spl-associated-token-account-interface`,`spl-memo-interface` | 6.0.0,3.3.0,3.1.0,4.0.0,5.1.1,7.3.0,3.0.0,4.0.0,2.0.0,2.1.0 | Apache-2.0                   | 整组 wasm 134,Android 117.新增 84                     |
| Solana 对照    | `@codama/dynamic-parsers`,RPC 的 `jsonParsed` 输出                                                                                                                                                                                                                                   | 随语料任务固定                                              | MIT                          | 不需要编译 Agave                                      |
| EVM,TRON 对照  | `viem`,`tronweb`                                                                                                                                                                                                                                                                     | 沿用 `apps/web` 锁定的版本或单独固定                        | MIT                          | 运行时已用 alloy 和 prost,对照必须是另一套实现        |
| Bitcoin 对照   | Bitcoin Core `decodepsbt`,`decoderawtransaction`                                                                                                                                                                                                                                     | CI 固定版本                                                 | MIT                          | 已用于 regtest 验证                                   |
| 模糊测试       | `cargo-fuzz`                                                                                                                                                                                                                                                                         | CI 固定版本和 nightly                                       | MIT OR Apache-2.0            | 只在 CI                                               |

官方 interface crate 不进产物,原因是:

- 新增 84 个 crate,比另外三条链的总和还多.`solana-address`,`solana-instruction`,`solana-instruction-error`,`solana-pubkey`,`solana-serialize-utils` 各解析出两个版本.
- `spl-token-2022-interface` 在 wasm 上引入 `getrandom 0.2`(`js`),`js-sys`,`wasm-bindgen`,`curve25519-dalek 4`.
- 它们没有字节范围,`unpack` 丢弃尾部字节,`unpack_with_rest` 是 `pub(crate)`.System,Stake,Vote 还要额外启用 `wincode` 或 `bincode`.
- 官方 Codama IDL 已经是机器可读的格式定义,核对过:System 14,Token 28,Token-2022 100,Compute Budget 5,Address Lookup Table 5 都和 interface crate 的枚举一一对应.IDL 用到的类型节点只有 23 种.同一个遍历器还能读第三方程序的描述文件.

### 6.3 Schema 来源

| 来源                                       | 固定方式                                                                     | 许可                                                  |
| ------------------------------------------ | ---------------------------------------------------------------------------- | ----------------------------------------------------- |
| java-tron `protocol/src/main/protos/core/` | tag `GreatVoyage-v4.8.2.4`,commit `4e8389f159d0782f1ebcd9dab2fd88388c0e6fe9` | 仓库 LGPL-3.0,`account_contract.proto` 文件头是 GPLv3 |
| `solana-program/*` 的 `idl.json`           | commit 和 SHA-256                                                            | Apache-2.0                                            |
| ERC-7730 描述文件                          | `ethereum/clear-signing-erc7730-registry` 的 commit 和 SHA-256               | CC0-1.0                                               |

### 6.4 固定方法

1. `Cargo.toml` 写精确版本 `=x.y.z`,`Cargo.lock` 入库,升级是单独的改动,要重跑语料.
2. 新增依赖在 `deny.toml` 写明理由,`cargo deny` 检查许可,来源和重复版本.
3. Schema 文件放在 `schemas/`,`schemas/lock.toml` 记录仓库,commit,路径,SHA-256.生成器先校验哈希.
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

| 规则 | 内容                                                                                                                              |
| ---- | --------------------------------------------------------------------------------------------------------------------------------- |
| 绑定 | 调用数据按链,目标地址,选择器匹配.EIP-712 按链,`verifyingContract`,`primaryType`,类型串匹配.`context` 的全部约束都要满足           |
| 解码 | 按描述里的 ABI 用 `alloy-dyn-abi` 解码,重新编码必须等于 `data`.不相等按没有描述处理                                               |
| 类别 | 描述文件只决定字段名和展示.类别由策略按解码出的内部动作判定,不能降低                                                              |
| 嵌套 | ERC-7730 的嵌入调用和 multicall 递归解码,最多 3 层,32 条.超过时按没有描述处理                                                     |
| 内置 | `transfer`,`approve`,Permit 等来自注册表的 `ercs/calldata-erc20-tokens.json`,`ercs/eip712-erc2612-permit.json`,只绑定清单内的合约 |
| 升级 | 描述绑定地址,不绑定代码.合约升级后含义可能改变,离线设备无法发现,所以描述文件给不出普通转账,普通转账只给清单内的代币               |

TRON 的 TRC-20 调用使用同一份 ABI 规则,地址按 TRON 的格式显示.

### 7.3 Solana

ERC-7730 没有 Solana 绑定.BitOne 的 Solana 描述文件由两部分组成:

- **结构**:Codama IDL,和第 5.3 节用同一个遍历器.Anchor IDL 用 `@codama/nodes-from-anchor` 转换.
- **展示**:和 ERC-7730 `display.formats` 同构的一段,键是指令名.

绑定是网络,程序 ID,判别值.程序 ID 可以升级,规则同 7.2 的"升级".

### 7.4 没有描述文件

| 链     | 显示                                                      |
| ------ | --------------------------------------------------------- |
| EVM    | 目标地址,选择器,`value`,按 32 字节分行的原始参数          |
| TRON   | 同上,加 `call_token_value`,`token_id`                     |
| Solana | 程序 ID,账户列表和签名者,可写标记,`data` 的长度和十六进制 |

类别是无法识别,长按签名.函数签名数据库不是可信来源,不用来猜测方法名.

### 7.5 清单格式

清单增加 `descriptors`,取代 `functions`.`functions` 目前是空的.描述文件在清单构建时编译成紧凑的二进制形式.内置清单没有大小限制,经二维码导入的清单包受 `MAX_MESSAGE_BYTES`(256 KiB)限制.

## 8. 验证

对照工具放在工作区之外的 `tools/crosscheck/`.普通 PR 的 CI 只读入库的语料快照,不访问网络.

### 8.1 官方向量和对照

| 链      | 官方向量                                                             | 独立对照                                                      |
| ------- | -------------------------------------------------------------------- | ------------------------------------------------------------- |
| EVM     | `ethereum/tests` 的 `TransactionTests`.EIP-712 规范示例              | `viem`,类型 0 到 4,EIP-712 摘要,EIP-7702 授权,`personal_sign` |
| TRON    | java-tron 主网 `txID`                                                | `tronweb`,它的 `*_pb.cjs` 由同一份 proto 生成                 |
| Solana  | `solana-program/*` 的 IDL 和 interface crate                         | interface crate,`@codama/dynamic-parsers`,RPC 的 `jsonParsed` |
| Bitcoin | BIP-143,BIP-174,BIP-341,BIP-370 向量,Bitcoin Core 的 `rpc_psbt.json` | Bitcoin Core `decodepsbt`                                     |

比对到字段级:对照工具解出的每个字段都要和树里同名节点相等.`tools/crosscheck/` 里的名字映射表是数据文件.

Bitcoin 的官方向量在 `tools/crosscheck/vectors/bitcoin.json`,`vectors.lock` 记录哈希,文件里写明 `bitcoin/bips` 的 commit.BIP-174,370,371 的 PSBT 向量和 `bitcoin`,`psbt-v2` 直接解出的结果逐字段比较,BIP-143,341 的向量比较 sighash.

EVM 的向量在 `tools/crosscheck/vectors/evm.json`,由 `tools/crosscheck/viem/generate.mjs` 生成,viem 的版本固定,名字映射表是 `tools/crosscheck/viem/names.json`.运行时用 alloy 解码,所以字段的期望值来自 viem.类型 0 到 4 用测试私钥由 viem 构建,`ethereum/tests` 的合法向量由 viem 解开,生成时核对官方的哈希和发送方.树里的字段,未签名编码,装回签名的字节,交易哈希,恢复出的发送方和授权人都要相同.官方的不合法向量去掉签名三项后必须解码失败.类型 3,4 没有官方向量进入快照.

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

## 9. 迁移顺序

前提:进行中的 sighash,收款告警,Permit2 工作先合并,它们的规则进入策略表.

新旧链路并存.`tx::review` 按链分发,`ReviewResult` 先增加 `fields`,保留旧字段.`fields` 非空时手机用通用列表,所以每条链可以单独发布.

| 步骤 | 内容                                                                                                   | 依赖 | 单独发布    |
| ---- | ------------------------------------------------------------------------------------------------------ | ---- | ----------- |
| 0    | 模型,C1 到 C4,风险顺序,`fields`,通用列表,CI 里的 wasm 和 Android 依赖构建,`tools/crosscheck/`,语料任务 | 无   | 是,行为不变 |
| 1    | Bitcoin:PSBT 键值遍历器,PSBT v0,脚本,`sequence`,`version`,`OP_RETURN`                                  | 0    | 是          |
| 2    | Bitcoin:PSBT v2                                                                                        | 1    | 是          |
| 3    | TRON:生成器,41 个合约类型,备注,`Permission_id`.`TriggerSmartContract` 暂走旧分类                       | 0    | 是          |
| 4    | EVM:类型 0 到 4,访问列表,授权列表.合约调用暂走旧分类                                                   | 0    | 是          |
| 5    | 应用层:清单 `descriptors`,ERC-20,TRC-20 内置描述,无描述按无法识别.删除 `abi.rs` 的手写分类             | 3,4  | 是          |
| 6    | EIP-712 通用解码                                                                                       | 4,5  | 是          |
| 7    | Solana:legacy,v0,v1,IDL 遍历器,System,Compute Budget,SPL Token,Token-2022 基础指令,Associated Token    | 0    | 是          |
| 8    | Solana:Memo,Address Lookup Table,Stake,Vote,Token-2022 扩展                                            | 7    | 是,按程序   |
| 9    | Solana 第三方描述文件                                                                                  | 5,7  | 是          |
| 10   | 删除 `Row`,`ActionView`,`RawLine`,`raw_lines` 和旧 FFI 字段,更新 `docs/03`,`04`,`07`,`08` 和文档索引   | 全部 | 是          |

- 第 7 步一次带上现有转账用到的全部程序,否则 SPL 转账会被当成第三方程序.
- 内置描述必须先于手写分类的删除.
- 每一步都走 CI 构建,不在开发机上跑批量测试.

| 位置                                     | 处理                                                                                                                                                                                                                                                        |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tx/tron.rs`,19 个                       | 保留 `varint_is_strict`,`round_trip`,未知字段,重复字段,字段顺序,负数,`exactly_one_contract`,`type_url`,地址前缀,有效期等.`permission_id_is_refused`,`call_token_value_is_refused` 改为解码并显示                                                            |
| `tx/solana.rs`,16 个                     | 消息层的全部保留.`unsupported_versions_are_refused` 改为未知版本可签,要二次确认                                                                                                                                                                             |
| `tx/abi.rs`,18 个                        | 输入保留,改成对内置描述运行.`approvals_are_red_on_any_contract` 改为清单外无法识别                                                                                                                                                                          |
| `tx/evm/typed.rs`,2 个                   | 保留                                                                                                                                                                                                                                                        |
| `tx/bitcoin.rs`,18 个                    | BIP-143,BIP-174,BIP-341 向量和选币测试保留                                                                                                                                                                                                                  |
| `review.rs`,4 个                         | `risk_is_the_highest_of_all_actions`,`forbidden_cannot_be_signed` 保留.`no_actions_is_never_treated_as_safe` 改为无法识别                                                                                                                                   |
| `bitone-signer/src/sign/evm.rs`,24 个    | 与 viem 的逐字节对比和第 1.1 节的拒绝保留.`unknown_contract_shows_raw_data`,`contract_creation_is_unknown` 保留.`other_typed_messages_are_refused` 改为解码后可签                                                                                           |
| `bitone-signer/src/sign/tron.rs`,19 个   | 与 tronweb 的对比保留.`other_system_contracts_are_unknown_and_show_raw_fields` 换成逐类型断言.`mutated_payloads_never_panic` 保留                                                                                                                           |
| `bitone-signer/src/sign/solana.rs`,67 个 | 与 web3.js 的对比,nonce,查找表,手续费保留.`revoke_and_close_are_unknown`,`legacy_token_transfer_is_unknown`,`other_system_instructions_are_unknown`,`token_outside_the_manifest_is_unknown_with_a_raw_amount` 改类别.尾部字节类测试改为 `opaque` 加无法识别 |
| `bitone-signer/src/sign/bitcoin.rs`,3 个 | 保留                                                                                                                                                                                                                                                        |
| `bitone-ffi/src/lib.rs`                  | `review_needs_no_pin_and_sign_rechecks`,`garbage_is_refused_not_an_error` 保留,断言改到新字段                                                                                                                                                               |
| Kotlin                                   | `SignFlowTest.kt`,`Fakes.kt` 随 `ReviewResult` 更新                                                                                                                                                                                                         |

## 10. 事实和假设

### 10.1 已核实

| 事实                                                                                           | 来源                                                                                                           |
| ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `ContractType` 有 41 个值,`raw` 和 `Contract` 的字段如第 5.2 节                                | java-tron `Tron.proto`,tag 和 master 一致,`develop` 的枚举相同                                                 |
| 20,32 没有消息定义和执行器,3 有消息没有执行器,51 有执行器                                      | java-tron `protos/core/contract/`,`actuator/` 目录                                                             |
| 备注按 `getMemoFee` 扣费,合约数必须为 1,哈希取 `getRawData().toByteArray()`                    | java-tron `Manager.java`,`TransactionCapsule.java`                                                             |
| 协议文件的许可                                                                                 | GitHub API,`account_contract.proto` 文件头                                                                     |
| SIMD-0385 状态 `Activated`,v1 的格式和限制,feature 公钥在 Agave 里                             | `solana-foundation/solana-improvement-documents`,`anza-xyz/agave` 的 `feature-set`                             |
| Solana 指令集合和 IDL 指令数                                                                   | `solana-program/*` 的 `idl.json`,对应 interface crate 的源码                                                   |
| EIP-7702 是 Final,EIP-8141 是 Draft,类型 0x06.`TxEip8141` 不是 `TxEnvelope` 的成员             | `ethereum/EIPs`,`alloy-consensus` 2.5.0 源码                                                                   |
| `TxLegacy::decode`,`TokenInstruction::unpack`,`prost` 的行为                                   | 对应 crate 的源码                                                                                              |
| crate 版本,许可,Rust 版本要求                                                                  | crates.io API                                                                                                  |
| 依赖量                                                                                         | `cargo tree`,工具链 1.95.0,只解析依赖,没有编译                                                                 |
| 上游 wasm 构建情况                                                                             | `alloy-rs/alloy`,`alloy-rs/core`,`anza-xyz/solana-sdk`,`solana-program/token-2022`,`tokio-rs/prost` 的 CI 文件 |
| ERC-7730 是 Draft,schema 版本 2.1.0,注册表已迁到 `ethereum/clear-signing-erc7730-registry`,CC0 | `ethereum/ERCs`,GitHub API                                                                                     |
| UniFFI 0.32.0 起支持递归记录                                                                   | `mozilla/uniffi-rs` 的 `CHANGELOG.md`                                                                          |
| PSBT 键的集合                                                                                  | BIP-174,370,371,373,375,353,127 和 `psbt-v2` 的 `consts.rs`.BIP-431,433 目前是 Draft                           |

### 10.2 假设,未验证

1. **构建**:`alloy-consensus`,`alloy-eips`,`alloy-primitives`,`alloy-rlp`,`psbt-v2` 已在 `bitone-core` 里构建.其余候选依赖的 wasm 和 Android 结论来自上游 CI 和依赖图,图里没有新增的 C 依赖.`solana-message`,`prost` 在 wasm 上没有直接的上游证据.
2. **体积**:没有测量 WASM 和 `.so` 的体积增长.
3. **主网日期**:v1 在 2026-09-15 前后激活来自二手报道,没有核对主网的 feature 账户.`Activated` 状态来自 SIMD 仓库.
4. **规范编码**:假设主网 TRON 交易都是规范字段顺序,`auths`,`scripts`,`provider`,`ContractName`,`ref_block_num` 没有合法用途.语料会验证,不成立时调整规则.
5. **类别**:矩阵里的风险类别是本标准的判断,不来自官方.
6. **IDL 准确性**:假设 IDL 和程序行为一致.`batch` 的内部指令在 IDL 里是字节数组,递归解码是遍历器的规则,不是 IDL 给出的.对照测试会验证.
7. **Solana 描述**:没有查到 Solana 上和 ERC-7730 对应的通用标准,这不等于不存在.
8. **其他链**:BSC,Polygon 对各交易类型的支持没有逐一核实,规则按 Chain ID 一视同仁.
9. **语料**:公开 RPC 的可用性和限流按现有实测估计,CI 上没有验证过.
10. **许可**:第 6 节的许可判断不是法律意见.
