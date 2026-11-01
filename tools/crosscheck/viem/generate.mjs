#!/usr/bin/env node
// 生成 EVM 对照向量,写到 ../vectors/evm.json.viem 是另一套实现,ethereum/tests 是官方向量.
// 需要网络,所以只在换向量时手动运行,CI 只读快照.
import { readFileSync, writeFileSync } from "node:fs";
import {
    concat,
    fromRlp,
    hashDomain,
    hashMessage,
    hashStruct,
    hashTypedData,
    keccak256,
    parseTransaction,
    recoverTransactionAddress,
    serializeTransaction,
    toHex,
    toRlp,
} from "viem";
import { privateKeyToAccount, sign } from "viem/accounts";
import { hashAuthorization, recoverAuthorizationAddress } from "viem/utils";

const COMMIT = "c67e485ff8b5be9abc8ad15345ec21aa22e290d9";
const BASE = `https://raw.githubusercontent.com/ethereum/tests/${COMMIT}/TransactionTests`;
const VALID = [
    "ttEIP2930/accessListStorage32Bytes",
    "ttNonce/TransactionWithHighNonce32",
    "ttVValue/V_equals37",
    "ttVValue/ValidChainID1ValidV0",
    "ttVValue/ValidChainID1ValidV1",
];
const INVALID = [
    "ttAddress/AddressLessThan20",
    "ttEIP1559/maxFeePerGas00prefix",
    "ttEIP1559/maxPriorityFeePerGas00prefix",
    "ttEIP2930/accessListAddressLessThan20",
    "ttEIP2930/accessListAddressPrefix00",
    "ttEIP2930/accessListStorage0x0001",
    "ttEIP2930/accessListStoragePrefix00",
    "ttNonce/TransactionWithLeadingZerosNonce",
];

// 格式合法,费用超过 128 位.alloy 的类型放不下,所以解码失败.
const TOO_WIDE = ["ttVValue/V_equals38"];

const names = JSON.parse(readFileSync(new URL("names.json", import.meta.url), "utf8"));
const viem = JSON.parse(readFileSync(new URL("package.json", import.meta.url), "utf8")).dependencies.viem;

// 公开的测试私钥,没有任何资产.
const KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const account = privateKeyToAccount(KEY);
const PEER = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const HASH_A = `0x${"11".repeat(32)}`;
const HASH_B = `0x${"22".repeat(32)}`;
const BLOBS = [`0x01${"33".repeat(31)}`, `0x01${"44".repeat(31)}`];
const ZERO = `0x${"00".repeat(20)}`;

const plain = (hex) => hex.replace(/^0x/, "").toLowerCase();

function field(key, value) {
    if (key === "to") return value ? plain(value) : names.contract_creation;
    if (key === "data") return plain(value ?? "0x");
    return String(value ?? 0);
}

/** 交易的字段,名字换成树里的名字.viem 省略值为 0 的字段. */
function fields(tx) {
    const always = ["nonce", "gas", "to", "value", "data"];
    const entries = Object.entries(names.fields).filter(([key]) => key in tx || always.includes(key));
    return Object.fromEntries(entries.map(([key, name]) => [name, field(key, tx[key])]));
}

async function authorizations(list) {
    return Promise.all(
        list.map(async (item) => ({
            chain_id: String(item.chainId),
            address: plain(item.address),
            nonce: String(item.nonce),
            y_parity: String(item.yParity),
            r: plain(item.r).padStart(64, "0"),
            s: plain(item.s).padStart(64, "0"),
            authority: plain(await recoverAuthorizationAddress({ authorization: item })),
        })),
    );
}

async function entry(name, origin, tx, signature, signed) {
    const unsigned = serializeTransaction(tx);
    const compact = concat([
        toHex(BigInt(signature.r), { size: 32 }),
        toHex(BigInt(signature.s), { size: 32 }),
        toHex(signature.yParity, { size: 1 }),
    ]);
    return {
        case: name,
        origin,
        unsigned: plain(unsigned),
        signature: plain(compact),
        signed: plain(signed),
        hash: plain(keccak256(signed)),
        sender: plain(await recoverTransactionAddress({ serializedTransaction: signed })),
        fields: fields(tx),
        access_list: (tx.accessList ?? []).map((item) => ({
            address: plain(item.address),
            storage_keys: item.storageKeys.map(plain),
        })),
        authorization_list: await authorizations(tx.authorizationList ?? []),
        blob_versioned_hashes: (tx.blobVersionedHashes ?? []).map(plain),
    };
}

async function own(name, tx) {
    const signature = await sign({ hash: keccak256(serializeTransaction(tx)), privateKey: KEY });
    return entry(name, "viem", tx, signature, serializeTransaction(tx, signature));
}

const fees = { maxFeePerGas: 30_000_000_000n, maxPriorityFeePerGas: 2_000_000_000n };
const accessList = [
    { address: PEER, storageKeys: [HASH_A, HASH_B] },
    { address: ZERO, storageKeys: [] },
];

async function ownTransactions() {
    const delegate = (chainId, address) => account.signAuthorization({ chainId, address, nonce: 5 });
    const delegations = await Promise.all([
        delegate(1, PEER),
        delegate(0, PEER),
        delegate(137, PEER),
        delegate(1, ZERO),
    ]);
    const base = { to: PEER, value: 1_500_000_000_000_000_000n, nonce: 7, gas: 21_000n };
    const cases = [
        ["旧式,没有 Chain ID", { ...base, type: "legacy", gasPrice: 1_000_000_000n }],
        ["旧式,EIP-155", { ...base, type: "legacy", chainId: 56, gasPrice: 3_000_000_000n, data: "0xdeadbeef" }],
        ["EIP-2930", { ...base, type: "eip2930", chainId: 1, gasPrice: 1_000_000_000n, accessList }],
        ["EIP-1559", { ...base, ...fees, type: "eip1559", chainId: 1, data: "0x01", accessList }],
        ["EIP-1559,创建合约", { ...fees, type: "eip1559", chainId: 137, nonce: 0, gas: 90_000n, data: "0x6080" }],
        [
            "EIP-4844",
            {
                ...base,
                ...fees,
                type: "eip4844",
                chainId: 1,
                maxFeePerBlobGas: 5n,
                blobVersionedHashes: BLOBS,
            },
        ],
        ["EIP-7702", { ...base, ...fees, type: "eip7702", chainId: 1, authorizationList: delegations }],
    ];
    return Promise.all(cases.map(([name, tx]) => own(name, tx)));
}

async function official(path) {
    const body = await fetch(`${BASE}/${path}.json`).then((response) => response.json());
    const [test] = Object.values(body);
    return test;
}

async function officialValid(path) {
    const test = await official(path);
    const signed = test.txbytes;
    const parsed = parseTransaction(signed);
    const { r, s, v, yParity, ...tx } = parsed;
    const parity = yParity ?? (v === 27n || v === 28n ? Number(v - 27n) : Number((v - 35n) % 2n));
    const vector = await entry(path, "ethereum/tests", tx, { r, s, yParity: parity }, signed);
    const expected = test.result.Prague;
    if (plain(expected.hash) !== vector.hash || plain(expected.sender) !== vector.sender) {
        throw new Error(`${path} 和官方结果不符`);
    }
    return vector;
}

/** 官方的不合法向量是带签名的,去掉 v,r,s 三项,其余字节原样保留. */
async function officialInvalid(path) {
    const { txbytes } = await official(path);
    const typed = !/^0x[c-f]/.test(txbytes);
    const body = typed ? `0x${txbytes.slice(4)}` : txbytes;
    const unsigned = toRlp(fromRlp(body, "hex").slice(0, -3));
    return { case: path, unsigned: plain(typed ? concat([txbytes.slice(0, 4), unsigned]) : unsigned) };
}

const domain = { name: "Tether", version: "1", chainId: 1, verifyingContract: PEER };
const types = {
    EIP712Domain: [
        { name: "name", type: "string" },
        { name: "version", type: "string" },
        { name: "chainId", type: "uint256" },
        { name: "verifyingContract", type: "address" },
    ],
    Permit: [
        { name: "owner", type: "address" },
        { name: "spender", type: "address" },
        { name: "value", type: "uint256" },
        { name: "nonce", type: "uint256" },
        { name: "deadline", type: "uint256" },
    ],
};
const message = { owner: account.address, spender: PEER, value: 1n, nonce: 0n, deadline: 2n };

function messages() {
    const typedParts = [
        "0x1901",
        hashDomain({ domain, types }),
        hashStruct({ data: message, primaryType: "Permit", types }),
    ];
    const tuple = { chainId: 1, address: PEER, nonce: 9 };
    const rlp = toRlp([toHex(tuple.chainId), tuple.address, toHex(tuple.nonce)]);
    return [
        {
            case: "EIP-712 Permit 摘要",
            kind: "transaction",
            payload: plain(concat(typedParts)),
            hash: plain(hashTypedData({ domain, types, primaryType: "Permit", message })),
        },
        {
            case: "EIP-7702 授权",
            kind: "transaction",
            payload: plain(concat(["0x05", rlp])),
            hash: plain(hashAuthorization(tuple)),
        },
        { case: "personal_sign", kind: "personal", payload: plain(toHex("hello")), hash: plain(hashMessage("hello")) },
        {
            case: "personal_sign 字节",
            kind: "personal",
            payload: "fffe",
            hash: plain(hashMessage({ raw: "0xfffe" })),
        },
    ];
}

const vectors = {
    source: {
        official: { repository: "ethereum/tests", commit: COMMIT, directory: "TransactionTests" },
        independent: { package: "viem", version: viem },
    },
    transactions: [...(await ownTransactions()), ...(await Promise.all(VALID.map(officialValid)))],
    invalid: await Promise.all(INVALID.map(officialInvalid)),
    too_wide: await Promise.all(TOO_WIDE.map(officialInvalid)),
    messages: messages(),
};
writeFileSync(new URL("../vectors/evm.json", import.meta.url), `${JSON.stringify(vectors, null, 4)}\n`);
