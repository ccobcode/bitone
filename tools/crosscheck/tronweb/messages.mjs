#!/usr/bin/env node
// 生成 TIP-191,TIP-712 和 TRC-20 调用的对照向量,写到 ../vectors/tron-messages.json.样本在 message-samples.json.
// 不访问网络.只在换向量时手动运行,CI 只读快照.见 docs/10-tx-decoding.md 第 8.1 节.
import { readFileSync, writeFileSync } from "node:fs";
import { utils } from "tronweb";

const samples = JSON.parse(readFileSync(new URL("message-samples.json", import.meta.url), "utf8"));
const version = JSON.parse(readFileSync(new URL("package.json", import.meta.url), "utf8")).dependencies.tronweb;

// BIP-39 规范的测试助记词,没有任何资产,和 bitone-signer 的测试相同.
const MNEMONIC = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const account = utils.accounts.generateAccountWithMnemonic(MNEMONIC, "m/44'/195'/0'/0/0");
const key = account.privateKey.replace(/^0x/, "");

const plain = (hex) => hex.replace(/^0x/, "").toLowerCase();
const { TypedDataEncoder } = utils.typedData;
const { keccak256, toUtf8Bytes } = utils.ethersUtils;

function personal({ case: label, text, hex }) {
    const message = hex === undefined ? text : [...Buffer.from(hex, "hex")];
    const signature = utils.message.signMessage(message, key);
    return {
        case: label,
        payload: Buffer.from(hex === undefined ? Buffer.from(text, "utf8") : Buffer.from(hex, "hex")).toString("hex"),
        hash: plain(utils.message.hashMessage(message)),
        signature: plain(signature),
    };
}

function typed({ case: label, domain, types, value }) {
    const encoder = new TypedDataEncoder(types);
    const fields = [
        ["name", "string"],
        ["version", "string"],
        ["chainId", "uint256"],
        ["verifyingContract", "address"],
    ].filter(([name]) => domain[name] !== undefined);
    const payload = {
        types: { EIP712Domain: fields.map(([name, type]) => ({ name, type })), ...types },
        primaryType: encoder.primaryType,
        domain,
        message: value,
    };
    return {
        case: label,
        payload: JSON.stringify(payload),
        domain_separator: plain(TypedDataEncoder.hashDomain(domain)),
        hash_struct: plain(encoder.hash(value)),
        hash: plain(TypedDataEncoder.hash(domain, types, value)),
        signature: plain(utils.typedData.signTypedData(domain, types, value, key)),
    };
}

function call({ case: label, name, types, args }) {
    const signature = `${name}(${types.join(",")})`;
    const selector = plain(keccak256(toUtf8Bytes(signature))).slice(0, 8);
    const encoded = plain(utils.abi.encodeParams(types, args));
    const decoded = utils.abi.decodeParams([], types, `0x${encoded}`);
    return {
        case: label,
        data: selector + encoded,
        fields: decoded.map((value, index) =>
            types[index] === "address" ? utils.address.fromHex(value) : String(value),
        ),
    };
}

const vectors = {
    source: { package: "tronweb", version },
    signer: account.address,
    personal: samples.personal.map(personal),
    typed: samples.typed.map(typed),
    calls: samples.calls.map(call),
};
writeFileSync(new URL("../vectors/tron-messages.json", import.meta.url), `${JSON.stringify(vectors, null, 4)}\n`);
console.log(`${vectors.personal.length} 条 TIP-191,${vectors.typed.length} 条 TIP-712,${vectors.calls.length} 个调用`);
