#!/usr/bin/env node
// 生成 TRON 对照向量,写到 ../vectors/tron.json.tronweb 的 *_pb.cjs 由同一份 proto 生成,是 prost 之外的另一套实现.
// tronweb 没有 market_contract 和 shield_contract,这三种合约用 google-protobuf 的 BinaryWriter 按编号写.
// 只在换向量时手动运行,CI 只读快照.见 docs/10-tx-decoding.md 第 8.1 节.
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { utils } from "tronweb";

// pnpm 的依赖是符号链接,google-protobuf 要从真实路径找.
const PB = realpathSync(fileURLToPath(new URL("node_modules/tronweb/lib/commonjs/protocol/core", import.meta.url)));
const load = createRequire(`${PB}/Tron_pb.cjs`);
const pb = Object.assign(
    {},
    ...[
        "Tron_pb.cjs",
        "contract/account_contract_pb.cjs",
        "contract/asset_issue_contract_pb.cjs",
        "contract/balance_contract_pb.cjs",
        "contract/common_pb.cjs",
        "contract/exchange_contract_pb.cjs",
        "contract/proposal_contract_pb.cjs",
        "contract/smart_contract_pb.cjs",
        "contract/storage_contract_pb.cjs",
        "contract/vote_asset_contract_pb.cjs",
        "contract/witness_contract_pb.cjs",
    ].map((file) => load(`./${file}`)),
);
const jspb = load("google-protobuf");
const { Any } = load("google-protobuf/google/protobuf/any_pb.js");

const samples = JSON.parse(readFileSync(new URL("samples.json", import.meta.url), "utf8"));
const version = JSON.parse(readFileSync(new URL("package.json", import.meta.url), "utf8")).dependencies.tronweb;

// BIP-39 规范的测试助记词,没有任何资产,和 bitone-signer 的测试相同.
const MNEMONIC = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const account = utils.accounts.generateAccountWithMnemonic(MNEMONIC, "m/44'/195'/0'/0/0");
const seen = { filled: new Set(), empty: new Set() };
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const unhex = (text) => Uint8Array.from(Buffer.from(text, "hex"));
const utf8 = (text) => Uint8Array.from(Buffer.from(text, "utf8"));
const OWNER = unhex(utils.address.toHex(account.address));
const PEER = unhex(utils.address.toHex("TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE"));
const PREFIX = "type.googleapis.com/protocol.";
const NOW = 1_800_000_000_000;
const REF_BLOCK = { bytes: unhex("abcd"), hash: unhex("0102030405060708") };

const pascal = (key) => key[0].toUpperCase() + key.slice(1);
const normal = (key) => key.replace(/_/g, "").toLowerCase();
const member = (path) => path.split(".").reduce((scope, name) => scope[name], pb);

function bytesOf(text) {
    if (text === "@owner") return OWNER;
    if (text === "@peer") return PEER;
    if (text.startsWith("hex:")) return unhex(text.slice(4));
    if (text.startsWith("text:")) return utf8(text.slice(5));
    throw new Error(`字节串 ${text}`);
}

/** jspb 的枚举键全是大写,官方的名字在样本里. */
const enumNumber = (path, name) => {
    const number = member(path)[name.toUpperCase()];
    if (number === undefined) throw new Error(`${path} 没有 ${name}`);
    return number;
};
const contractNumber = (name) => enumNumber("Transaction.Contract.ContractType", name);

const isString = (text) => typeof text === "string" && text.startsWith("str:");

function scalar(value) {
    if (isString(value)) return value.slice(4);
    if (typeof value === "string") return bytesOf(value);
    if (value?.$enum) return enumNumber(value.$enum, value.name);
    if (value?.$type) return build(value);
    return value;
}

function build(spec) {
    const message = new (member(spec.$type))();
    for (const [key, value] of Object.entries(spec)) {
        if (key === "$type") continue;
        const name = pascal(key);
        if (value?.$map) {
            const map = message[`get${name}Map`]();
            value.$map.forEach(([entry, content]) => map.set(entry, content));
        } else if (Array.isArray(value)) {
            message[`set${name}List`](value.map(scalar));
        } else {
            message[`set${name}`](scalar(value));
        }
    }
    return message;
}

function shown(message, key, value) {
    const name = pascal(key);
    if (value?.$map) {
        const entries = [...message[`get${name}Map`]().entries()];
        return entries.map(({ 0: entry, 1: content }) => ({ key: String(entry), value: String(content) }));
    }
    if (Array.isArray(value)) {
        const items = typeof value[0] === "string" ? message[`get${name}List_asU8`]() : message[`get${name}List`]();
        return items.map((item, index) => (item.serializeBinary ? read(item, value[index]) : hex(item)));
    }
    if (isString(value)) return hex(utf8(message[`get${name}`]()));
    if (typeof value === "string") return hex(message[`get${name}_asU8`]());
    if (value?.$enum) {
        if (enumNumber(value.$enum, value.name) !== message[`get${name}`]()) throw new Error(`${key} 的枚举不符`);
        return value.name;
    }
    if (value?.$type) return read(message[`get${name}`](), value);
    return String(message[`get${name}`]());
}

function read(message, spec) {
    const entries = Object.entries(spec).filter(([key]) => key !== "$type");
    return Object.fromEntries(entries.map(([key, value]) => [normal(key), shown(message, key, value)]));
}

/** 记下这条消息填了和没填的字段.全部样本合起来,tronweb 认识的字段都要填过,这样对照才是全部字段. */
function coverage(message, spec, seen) {
    for (const [key, value] of Object.entries(message.toObject())) {
        const name = key.replace(/(List|Map)$/, "");
        const field = `${spec.$type}.${name}`;
        (name in spec ? seen.filled : seen.empty).add(field);
        const nested = spec[name];
        if (nested?.$type && value) coverage(message[`get${pascal(name)}`](), nested, seen);
        if (Array.isArray(nested) && nested[0]?.$type) {
            message[`get${pascal(name)}List`]().forEach((item, index) => {
                coverage(item, nested[index], seen);
            });
        }
    }
}

function wireMessage(fields) {
    const writer = new jspb.BinaryWriter();
    for (const [number, , kind, value] of fields) {
        if (kind === "int64") writer.writeInt64(number, value);
        if (kind === "bytes") writer.writeBytes(number, bytesOf(value));
        if (kind === "repeated") writer.writeBytes(number, wireMessage(value));
    }
    return writer.getResultBuffer();
}

function wireShown(fields) {
    const pairs = fields.map(([, name, kind, value]) => {
        if (kind === "repeated") return [normal(name), [wireShown(value)]];
        return [normal(name), kind === "int64" ? String(value) : hex(bytesOf(value))];
    });
    return Object.fromEntries(pairs);
}

function contractValue(name) {
    if (samples.contracts[name]) {
        const spec = { $type: name, ...samples.contracts[name] };
        const message = build(spec);
        const bytes = message.serializeBinary();
        const parsed = member(name).deserializeBinary(bytes);
        coverage(parsed, spec, seen);
        return { bytes, shown: read(parsed, spec) };
    }
    if (samples.wire[name]) {
        return { bytes: wireMessage(samples.wire[name]), shown: wireShown(samples.wire[name]) };
    }
    if (samples.no_message.includes(name)) {
        const bytes = unhex("0102");
        return { bytes, shown: hex(bytes) };
    }
    throw new Error(`没有 ${name} 的样本`);
}

/** `extra` 是 `Transaction.raw` 和 `Contract` 上额外的字段. */
function rawMessage(number, typeUrl, value, extra = {}) {
    const any = new Any();
    any.setTypeUrl(typeUrl);
    any.setValue(value);
    const contract = new pb.Transaction.Contract();
    contract.setType(number);
    contract.setParameter(any);
    if (extra.permission) contract.setPermissionId(extra.permission);
    if (extra.provider) contract.setProvider(extra.provider);
    if (extra.contractName) contract.setContractname(extra.contractName);
    const raw = new pb.Transaction.raw();
    raw.setRefBlockBytes(REF_BLOCK.bytes);
    raw.setRefBlockHash(REF_BLOCK.hash);
    raw.setExpiration(NOW + 1_800_000);
    raw.setContractList([contract]);
    raw.setTimestamp(NOW);
    if (extra.feeLimit) raw.setFeeLimit(extra.feeLimit);
    if (extra.memo) raw.setData(extra.memo);
    if (extra.refBlockNum) raw.setRefBlockNum(extra.refBlockNum);
    if (extra.scripts) raw.setScripts(extra.scripts);
    if (extra.auths) raw.setAuthsList(extra.auths.map(scalar));
    return raw;
}

function rawShown(raw, name, value, extra = {}) {
    const contract = raw.getContractList()[0];
    const any = contract.getParameter();
    const shownContract = {
        type: name,
        parameter: { typeurl: hex(utf8(any.getTypeUrl())), value },
    };
    if (extra.permission) shownContract.permissionid = String(contract.getPermissionId());
    if (extra.provider) shownContract.provider = hex(contract.getProvider_asU8());
    if (extra.contractName) shownContract.contractname = hex(contract.getContractname_asU8());
    const fields = {
        refblockbytes: hex(raw.getRefBlockBytes_asU8()),
        refblockhash: hex(raw.getRefBlockHash_asU8()),
        expiration: String(raw.getExpiration()),
        contract: [shownContract],
        timestamp: String(raw.getTimestamp()),
    };
    if (extra.feeLimit) fields.feelimit = String(raw.getFeeLimit());
    if (extra.memo) fields.data = hex(raw.getData_asU8());
    if (extra.refBlockNum) fields.refblocknum = String(raw.getRefBlockNum());
    if (extra.scripts) fields.scripts = hex(raw.getScripts_asU8());
    if (extra.auths) {
        fields.auths = raw.getAuthsList().map((item, index) => read(item, extra.auths[index]));
    }
    return fields;
}

function signedVector(label, contract, bytes, fields, owner) {
    const txID = utils.ethersUtils.sha256(bytes).replace(/^0x/, "");
    const key = account.privateKey.replace(/^0x/, "");
    const signature = utils.crypto.signTransaction(key, { txID }).signature[0].toLowerCase();
    return {
        case: label,
        contract,
        raw: hex(bytes),
        txid: txID,
        signature,
        signed: hex(
            writer((output) => {
                output.writeBytes(1, bytes);
                output.writeBytes(2, unhex(signature));
            }),
        ),
        owner,
        fields,
    };
}

function contractVector(name, number) {
    const { bytes: value, shown: expected } = contractValue(name);
    const raw = rawMessage(number, `${PREFIX}${name}`, value);
    const bytes = raw.serializeBinary();
    const parsed = pb.Transaction.raw.deserializeBinary(bytes);
    const owner = samples.no_message.includes(name) ? null : account.address;
    return signedVector(name, name, bytes, rawShown(parsed, name, expected), owner);
}

function special(label, name, number, extra) {
    const { bytes: value, shown: expected } = contractValue(name);
    const bytes = rawMessage(number, `${PREFIX}${name}`, value, extra).serializeBinary();
    const parsed = pb.Transaction.raw.deserializeBinary(bytes);
    return signedVector(label, name, bytes, rawShown(parsed, name, expected, extra), account.address);
}

function concat(...parts) {
    return Uint8Array.from(parts.flatMap((part) => [...part]));
}

function writer(write) {
    const output = new jspb.BinaryWriter();
    write(output);
    return output.getResultBuffer();
}

/** 以后的 java-tron 加的字段.tronweb 只跳过,所以这里写在已知字段后面,编号比它们都大. */
function unknownVectors() {
    const name = "TransferContract";
    const number = contractNumber(name);
    const { bytes: value, shown: expected } = contractValue(name);
    const url = `${PREFIX}${name}`;
    const tail = writer((output) => output.writeInt64(99, 1));
    const top = concat(rawMessage(number, url, value).serializeBinary(), tail);
    const parsed = pb.Transaction.raw.deserializeBinary(top);
    const inner = concat(
        value,
        writer((output) => output.writeBytes(9, unhex("0708"))),
    );
    const nested = rawMessage(number, url, inner).serializeBinary();
    const future = rawMessage(99, `${PREFIX}FutureContract`, unhex("010203")).serializeBinary();
    const fields = rawShown(parsed, name, expected);
    const innerFields = { ...expected, field9: "0708" };
    const futureRaw = pb.Transaction.raw.deserializeBinary(future);
    return [
        signedVector("Transaction.raw 的未知字段", name, top, { ...fields, field99: "01" }, account.address),
        signedVector(
            "TransferContract 的未知字段",
            name,
            nested,
            rawShown(pb.Transaction.raw.deserializeBinary(nested), name, innerFields),
            account.address,
        ),
        signedVector("未知合约类型", null, future, rawShown(futureRaw, "99", "010203"), null),
    ];
}

function refusedVectors() {
    const name = "TransferContract";
    const number = contractNumber(name);
    const url = `${PREFIX}${name}`;
    const { bytes: value } = contractValue(name);
    const nonMinimal = concat(value.slice(0, value.length - 4), Uint8Array.of(0x18, 0xe0, 0xc6, 0xdb, 0x80, 0x00));
    const repeated = concat(
        value,
        writer((output) => output.writeInt64(3, 1)),
    );
    const explicit = concat(rawMessage(number, url, value).serializeBinary(), Uint8Array.of(0x90, 0x01, 0x00));
    const swapped = (() => {
        const raw = rawMessage(number, url, value);
        const contract = raw.getContractList()[0].serializeBinary();
        return writer((output) => {
            output.writeBytes(1, REF_BLOCK.bytes);
            output.writeBytes(4, REF_BLOCK.hash);
            output.writeInt64(8, raw.getExpiration());
            output.writeInt64(14, raw.getTimestamp());
            output.writeBytes(11, contract);
        });
    })();
    const twice = rawMessage(number, url, value);
    twice.addContract(twice.getContractList()[0]);
    const asset = new pb.AssetIssueContract();
    asset.setId("1002000");
    asset.setOwnerAddress(OWNER);
    const issued = rawMessage(
        contractNumber("AssetIssueContract"),
        `${PREFIX}AssetIssueContract`,
        asset.serializeBinary(),
    );
    return [
        ["varint 不是最短编码", rawMessage(number, url, nonMinimal)],
        ["标量字段写了两次", rawMessage(number, url, repeated)],
        ["显式写默认值", explicit],
        ["字段乱序", swapped],
        ["type_url 和 type 不符", rawMessage(number, `${PREFIX}TriggerSmartContract`, value)],
        ["未知编号声明已知合约", rawMessage(99, url, value)],
        ["两个合约", twice],
        ["tronweb 把编号 41 写在最前面,节点按编号重新序列化", issued],
    ].map(([label, raw]) => ({ case: label, raw: hex(raw.serializeBinary?.() ?? raw) }));
}

const names = [...Object.keys(samples.contracts), ...Object.keys(samples.wire), ...samples.no_message];
if (names.length !== Object.keys(pb.Transaction.Contract.ContractType).length) throw new Error("样本少了合约类型");
const built = names.map((name) => contractVector(name, contractNumber(name)));
const unset = [...seen.empty].filter((field) => !seen.filled.has(field));
if (JSON.stringify(unset) !== JSON.stringify(samples.not_filled)) throw new Error(`没有填过的字段 ${unset}`);
const vectors = {
    source: {
        package: "tronweb",
        version,
        proto: "java-tron GreatVoyage-v4.8.2.4",
        by_field_number: Object.keys(samples.wire),
        not_filled: unset,
    },
    owner: account.address,
    transactions: [
        ...built,
        special("备注,权限和手续费上限", "TransferContract", contractNumber("TransferContract"), {
            memo: utf8("备注"),
            permission: 2,
            feeLimit: 100_000_000,
        }),
        special("链上不使用的字段", "TransferContract", contractNumber("TransferContract"), {
            refBlockNum: 7,
            scripts: unhex("aa"),
            provider: unhex("bb"),
            contractName: unhex("cc"),
            auths: [
                {
                    $type: "authority",
                    account: { $type: "AccountId", name: "text:acc", address: "@peer" },
                    permissionName: "text:active",
                },
            ],
        }),
        ...unknownVectors(),
    ],
    refused: refusedVectors(),
};
writeFileSync(new URL("../vectors/tron.json", import.meta.url), `${JSON.stringify(vectors, null, 4)}\n`);
console.log(`${vectors.transactions.length} 笔交易,${vectors.refused.length} 个拒绝`);
