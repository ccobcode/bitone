#!/usr/bin/env node
// 生成合约调用和 EIP-712 的对照向量,写到 ../vectors/evm-messages.json.样本在 samples.json.
// 不访问网络.只在换向量时手动运行,CI 只读快照.
import { readFileSync, writeFileSync } from "node:fs";
import {
    decodeFunctionData,
    encodeFunctionData,
    hashDomain,
    hashStruct,
    hashTypedData,
    parseAbi,
    toFunctionSignature,
} from "viem";

const samples = JSON.parse(readFileSync(new URL("samples.json", import.meta.url), "utf8"));
const version = JSON.parse(readFileSync(new URL("package.json", import.meta.url), "utf8")).dependencies.viem;

const plain = (hex) => hex.replace(/^0x/, "").toLowerCase();

/** 内部调用写成 `{abi, args}`,先编码成 `bytes`. */
function encoded(call) {
    const abi = parseAbi([call.abi]);
    const args = call.args.map(function argument(value) {
        if (Array.isArray(value)) return value.map(argument);
        return typeof value === "object" ? encoded(value) : value;
    });
    return encodeFunctionData({ abi, args });
}

/** 样本里的内部调用展开成它自己的字段,顺序和树里的叶子相同. */
function leaves(call) {
    const abi = parseAbi([call.abi]);
    const { args } = decodeFunctionData({ abi, data: encoded(call) });
    return args.flatMap(function leaf(value, index) {
        const raw = call.args[index];
        if (Array.isArray(value)) {
            return value.flatMap((item, at) => (typeof raw[at] === "object" ? leaves(raw[at]) : [String(item)]));
        }
        return [String(value)];
    });
}

const shown = (text) => (/^0x/.test(text) ? plain(text) : text);

const calls = samples.calls.map((call) => ({
    case: call.case,
    data: plain(encoded(call)),
    fields: leaves(call).map(shown),
}));

const irregular = samples.irregular.map((call) => ({
    case: call.case,
    data: plain(encoded(call)) + call.suffix,
    signature: toFunctionSignature(parseAbi([call.abi])[0]),
    fields: [...leaves(call).map(shown), call.suffix],
}));

function digest({ case: label, types, primaryType, domain, message }) {
    const hash = hashTypedData({ types, primaryType, domain, message });
    const full = {
        EIP712Domain: [
            ["name", "string"],
            ["version", "string"],
            ["chainId", "uint256"],
            ["verifyingContract", "address"],
            ["salt", "bytes32"],
        ]
            .filter(([name]) => domain[name] !== undefined)
            .map(([name, type]) => ({ name, type })),
        ...types,
    };
    return {
        case: label,
        payload: JSON.stringify({ types, primaryType, domain, message }),
        domain_separator: plain(hashDomain({ domain, types: full })),
        hash_struct:
            primaryType === "EIP712Domain" ? null : plain(hashStruct({ data: message, primaryType, types: full })),
        hash: plain(hash),
    };
}

const vectors = {
    source: { package: "viem", version },
    calls,
    irregular,
    typed: samples.typed.map(digest),
};
writeFileSync(new URL("../vectors/evm-messages.json", import.meta.url), `${JSON.stringify(vectors, null, 4)}\n`);
console.log(`${calls.length} 个调用,${irregular.length} 个不合规调用,${vectors.typed.length} 条 EIP-712`);
