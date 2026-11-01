// 由 schemas/solana/ 生成核心库的 IDL 表和 Solana 对照向量,见 docs/10-tx-decoding.md 第 6.4 节和第 8 节.
//
// 运行时用 bitone-core 的遍历器解码,所以字段的期望值来自 @codama/dynamic-parsers.
// 指令的字节由 @codama/dynamic-codecs 从 IDL 编码,样本值按类型节点确定地生成.

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { getNodeCodec } from "@codama/dynamic-codecs";
import { parseInstructionData } from "@codama/dynamic-parsers";
import { createFromJson } from "codama";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..", "..");
const SCHEMAS = join(ROOT, "schemas");
const SOLANA_PREFIX = "solana/";
const PROGRAMS = JSON.parse(readFileSync(join(HERE, "programs.json"), "utf8"));
const ASSET = join(ROOT, "crates", "bitone-core", "assets", "solana-idl.json");
const VECTORS = join(HERE, "..", "vectors", "solana.json");
const VECTORS_LOCK = join(HERE, "..", "vectors", "vectors.lock");
const VECTORS_NAME = "solana.json";
const BYTES_ENCODING = "base64";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const unhex = (text) => Uint8Array.from(Buffer.from(text, "hex"));

/** `schemas/lock.toml` 里的条目.只有 `key = "value"` 和表头两种行. */
function lockEntries(text) {
    const entries = [];
    for (const raw of text.split("\n")) {
        const line = raw.trim();
        if (line.startsWith("[")) {
            entries.push({});
            continue;
        }
        const match = /^(\w+) = "(.*)"$/.exec(line);
        if (match) entries[entries.length - 1][match[1]] = match[2];
    }
    return entries;
}

function loadSchemas() {
    const lock = lockEntries(readFileSync(join(SCHEMAS, "lock.toml"), "utf8"));
    return PROGRAMS.map((entry) => {
        const file = SOLANA_PREFIX + entry.file;
        const locked = lock.find((item) => item.file === file);
        if (!locked) throw new Error(`lock.toml 没有 ${file}`);
        const bytes = readFileSync(join(SCHEMAS, file));
        if (sha256(bytes) !== locked.sha256) throw new Error(`${file} 的哈希和 lock.toml 不符`);
        return { ...entry, path: file, sha256: locked.sha256, root: JSON.parse(bytes) };
    });
}

/** 遍历器认识的类型节点.新节点在这里失败,不会静默进入资产文件. */
function compactType(node, links) {
    const item = (child) => compactType(child, links);
    switch (node.kind) {
        case "numberTypeNode": {
            if (node.endian !== "le") throw new Error(`大端数字 ${node.format}`);
            const display = compactDisplay(node.display);
            return { kind: node.kind, format: node.format, ...(display ? { display } : {}) };
        }
        case "publicKeyTypeNode":
        case "bytesTypeNode":
            return { kind: node.kind };
        case "booleanTypeNode":
            return { kind: node.kind, size: item(node.size) };
        case "stringTypeNode":
            if (node.encoding !== "utf8") throw new Error(`字符串编码 ${node.encoding}`);
            return { kind: node.kind };
        case "sizePrefixTypeNode":
            return { kind: node.kind, prefix: item(node.prefix), type: item(node.type) };
        case "fixedSizeTypeNode":
            return { kind: node.kind, size: node.size, type: item(node.type) };
        case "optionTypeNode":
            if (node.fixed) throw new Error("定长的 option");
            return { kind: node.kind, prefix: item(node.prefix), item: item(node.item) };
        case "zeroableOptionTypeNode":
            if (node.zeroValue) throw new Error("自定义零值的 option");
            return { kind: node.kind, item: item(node.item) };
        case "arrayTypeNode":
            return { kind: node.kind, item: item(node.item), count: compactCount(node.count, links) };
        case "structTypeNode":
            return {
                kind: node.kind,
                fields: node.fields.map((field) => ({ name: field.name, type: item(field.type) })),
            };
        case "tupleTypeNode":
            return { kind: node.kind, items: node.items.map(item) };
        case "enumTypeNode":
            return {
                kind: node.kind,
                size: item(node.size),
                variants: node.variants.map((variant) => compactVariant(variant, links)),
            };
        case "definedTypeLinkNode":
            links.add(node.name);
            return { kind: node.kind, name: node.name };
        default:
            throw new Error(`遍历器不认识 ${node.kind}`);
    }
}

function compactCount(node, links) {
    switch (node.kind) {
        case "fixedCountNode":
            return { kind: node.kind, value: node.value };
        case "prefixedCountNode":
            return { kind: node.kind, prefix: compactType(node.prefix, links) };
        case "remainderCountNode":
            return { kind: node.kind };
        default:
            throw new Error(`遍历器不认识 ${node.kind}`);
    }
}

function compactVariant(variant, links) {
    const discriminator = variant.discriminator === undefined ? {} : { discriminator: variant.discriminator };
    switch (variant.kind) {
        case "enumEmptyVariantTypeNode":
            return { kind: variant.kind, name: variant.name, ...discriminator };
        case "enumStructVariantTypeNode":
            return {
                kind: variant.kind,
                name: variant.name,
                ...discriminator,
                struct: compactType(variant.struct, links),
            };
        case "enumTupleVariantTypeNode":
            return {
                kind: variant.kind,
                name: variant.name,
                ...discriminator,
                tuple: compactType(variant.tuple, links),
            };
        default:
            throw new Error(`遍历器不认识 ${variant.kind}`);
    }
}

/** 小数位由另一个参数给出时不展开,金额显示原始整数,清单内的代币由策略换算. */
function compactDisplay(display) {
    if (display?.kind === "dateTimeNumberDisplayNode") return { time: true };
    if (display?.kind !== "amountNumberDisplayNode") return undefined;
    const decimals = display.decimals?.kind === "numberValueNode" ? display.decimals.number : undefined;
    const unit = display.unit?.string;
    return decimals === undefined && unit === undefined ? undefined : { decimals, unit };
}

const NUMBER_BYTES = { u8: 1, u16: 2, u32: 4, u64: 8, u128: 16 };

function defaultBytes(argument) {
    const { type, defaultValue } = argument;
    if (defaultValue.kind === "bytesValueNode" && defaultValue.encoding === "base16") {
        return [...unhex(defaultValue.data)];
    }
    if (defaultValue.kind === "numberValueNode") {
        const size = NUMBER_BYTES[type.format];
        return [
            ...Buffer.from(
                BigInt(defaultValue.number)
                    .toString(16)
                    .padStart(size * 2, "0"),
                "hex",
            ),
        ].reverse();
    }
    throw new Error(`判别值的种类 ${defaultValue.kind}`);
}

function compactDiscriminator(node, instruction) {
    if (node.kind !== "fieldDiscriminatorNode") throw new Error(`判别方式 ${node.kind}`);
    const argument = instruction.arguments.find((item) => item.name === node.name);
    return { offset: node.offset, bytes: defaultBytes(argument) };
}

/** 签名者一组在前.没有这个标记的一组收下后面全部账户. */
function compactRemaining(node) {
    if (node.value.kind !== "argumentValueNode") throw new Error(`剩余账户 ${node.value.kind}`);
    return { name: node.value.name, signer: node.isSigner === true };
}

function compactInstruction(instruction, links) {
    return {
        name: instruction.name,
        accounts: (instruction.accounts ?? []).map((account) => ({
            name: account.name,
            optional: account.isOptional === true,
        })),
        arguments: instruction.arguments.map((argument) => ({
            name: argument.name,
            type: compactType(argument.type, links),
        })),
        discriminators: (instruction.discriminators ?? []).map((node) => compactDiscriminator(node, instruction)),
        remaining: (instruction.remainingAccounts ?? []).map(compactRemaining),
    };
}

function batchShape({ instruction, argument, accounts, payload }) {
    return { instruction, argument, accounts, payload };
}

/** 只留下指令用到的类型. */
function compactProgram(entry) {
    const { program } = entry.root;
    const links = new Set();
    const instructions = program.instructions.map((instruction) => compactInstruction(instruction, links));
    const definedTypes = {};
    const pending = [...links];
    while (pending.length) {
        const name = pending.pop();
        if (definedTypes[name]) continue;
        const found = new Set();
        const defined = program.definedTypes.find((item) => item.name === name);
        definedTypes[name] = compactType(defined.type, found);
        pending.push(...found);
    }
    return {
        name: program.name,
        ids: entry.ids ?? [program.publicKey],
        ...(entry.emptyData ? { emptyData: entry.emptyData } : {}),
        ...(entry.batch ? { batch: batchShape(entry.batch) } : {}),
        definedTypes,
        instructions,
    };
}

function writeAsset(schemas) {
    const asset = {
        sources: Object.fromEntries(schemas.map((entry) => [entry.path, entry.sha256])),
        programs: schemas.map(compactProgram),
    };
    writeFileSync(ASSET, `${JSON.stringify(asset)}\n`);
    return asset;
}

// 样本值.确定地由序号算出,生成结果可以逐字节复现.

const key = (seed) => {
    const bytes = sha256(`bitone-crosscheck-${seed}`);
    return unhex(bytes);
};

const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function base58(bytes) {
    let number = BigInt(`0x${hex(bytes)}`);
    let text = "";
    while (number > 0n) {
        text = BASE58[Number(number % 58n)] + text;
        number /= 58n;
    }
    for (const byte of bytes) {
        if (byte !== 0) break;
        text = `1${text}`;
    }
    return text;
}

const NUMBER_SAMPLES = {
    u8: (seed) => seed % 200,
    u16: (seed) => 1000 + seed,
    u32: (seed) => 100000 + seed,
    u64: (seed) => BigInt(5000000000 + seed),
    u128: (seed) => (1n << 100n) + BigInt(seed),
    i8: (seed) => -(seed % 100) - 1,
    i16: (seed) => -1000 - seed,
    i32: (seed) => -100000 - seed,
    i64: (seed) => BigInt(1700000000 + seed),
    i128: (seed) => -((1n << 100n) + BigInt(seed)),
    f32: (seed) => 1.5 + seed,
    f64: (seed) => 2.25 + seed,
};

/** 按类型节点给出样本值,形状是 `@codama/dynamic-codecs` 的输入. */
function sample(node, seed, types) {
    const next = (offset) => sample(node.item, seed + offset, types);
    switch (node.kind) {
        case "numberTypeNode":
            return NUMBER_SAMPLES[node.format](seed);
        case "publicKeyTypeNode":
            return base58(key(`value-${seed}`));
        case "bytesTypeNode":
            return ["base16", hex(key(`bytes-${seed}`).slice(0, 4 + (seed % 5)))];
        case "booleanTypeNode":
            return seed % 2 === 0;
        case "stringTypeNode":
            return `sample ${seed}`;
        case "sizePrefixTypeNode":
            return sample(node.type, seed, types);
        case "fixedSizeTypeNode":
            return node.type.kind === "bytesTypeNode"
                ? [
                      "base16",
                      hex(
                          Buffer.concat(
                              Array.from({ length: Math.ceil(node.size / 32) }, (_, index) =>
                                  key(`fixed-${seed}-${index}`),
                              ),
                          ).subarray(0, node.size),
                      ),
                  ]
                : sample(node.type, seed, types);
        case "optionTypeNode":
        case "zeroableOptionTypeNode":
            return seed % 3 === 0 ? { __option: "None" } : { __option: "Some", value: next(1) };
        case "arrayTypeNode":
            return Array.from(
                { length: node.count.kind === "fixedCountNode" ? node.count.value : 1 + (seed % 3) },
                (_, index) => next(index),
            );
        case "structTypeNode":
            return Object.fromEntries(
                node.fields.map((field, index) => [field.name, sample(field.type, seed + index + 1, types)]),
            );
        case "tupleTypeNode":
            return node.items.map((item, index) => sample(item, seed + index + 1, types));
        case "enumTypeNode":
            return sampleEnum(node, seed, types);
        case "definedTypeLinkNode":
            return sample(types[node.name], seed, types);
        default:
            throw new Error(`没有 ${node.kind} 的样本`);
    }
}

function sampleEnum(node, seed, types) {
    const variant = node.variants[seed % node.variants.length];
    const kind = { __kind: pascal(variant.name) };
    switch (variant.kind) {
        case "enumEmptyVariantTypeNode":
            return kind;
        case "enumStructVariantTypeNode":
            return { ...kind, ...sample(variant.struct, seed + 1, types) };
        case "enumTupleVariantTypeNode":
            return { ...kind, fields: sample(variant.tuple, seed + 1, types) };
        default:
            throw new Error(`没有 ${variant.kind} 的样本`);
    }
}

const pascal = (text) => text[0].toUpperCase() + text.slice(1);

/** 编码器不会自己填默认值. */
function omittedValue(value) {
    switch (value.kind) {
        case "numberValueNode":
            return value.number;
        case "bytesValueNode":
            return [value.encoding, value.data];
        default:
            throw new Error(`默认值的种类 ${value.kind}`);
    }
}

function instructionBytes(path, instruction, seed, types) {
    const values = {};
    instruction.arguments.forEach((argument, index) => {
        values[argument.name] =
            argument.defaultValueStrategy === "omitted"
                ? omittedValue(argument.defaultValue)
                : sample(argument.type, seed + index + 1, types);
    });
    return getNodeCodec(path).encode(values);
}

/** 对照工具解出的字段,路径是最近的字段名.`None` 没有值. */
function flatten(value, path, fields) {
    if (value === null || value === undefined) return;
    if (Array.isArray(value)) {
        if (value.length === 2 && value[0] === BYTES_ENCODING)
            fields.push([path, Buffer.from(value[1], BYTES_ENCODING).toString("hex")]);
        else value.forEach((item) => flatten(item, path, fields));
    } else if (typeof value === "object") {
        flattenObject(value, path, fields);
    } else {
        fields.push([path, String(value)]);
    }
}

function flattenObject(value, path, fields) {
    if (value.__option === "None") {
        fields.push([path, null]);
        return;
    }
    if (value.__option === "Some") {
        flatten(value.value, path, fields);
        return;
    }
    if (value.__kind) fields.push([path, value.__kind[0].toLowerCase() + value.__kind.slice(1)]);
    for (const [name, item] of Object.entries(value)) {
        if (name.startsWith("__")) continue;
        flatten(item, name === "fields" ? path : name, fields);
    }
}

/** `batch` 的内部指令在 IDL 里是字节数组,样本用同一份 IDL 编码真实的指令. */
function batchVector(entry, root, instruction, seed, types) {
    const inner = entry.batch.samples.map((name, index) => {
        const found = root.program.instructions.find((item) => item.name === name);
        return vector(entry, root, found, seed + index + 1, types);
    });
    const items = inner.map((item) => ({
        [entry.batch.accounts]: item.accounts.length,
        [entry.batch.payload]: [BYTES_ENCODING, Buffer.from(unhex(item.data)).toString(BYTES_ENCODING)],
    }));
    const discriminator = instruction.arguments.find((argument) => argument.defaultValueStrategy === "omitted");
    const values = { [discriminator.name]: discriminator.defaultValue.number, [entry.batch.argument]: items };
    const bytes = getNodeCodec([root, root.program, instruction]).encode(values);
    const parsed = parseInstructionData(root, bytes);
    if (parsed?.path.at(-1).name !== instruction.name) throw new Error(`对照工具认不出 ${instruction.name}`);
    const fields = [[discriminator.name, String(discriminator.defaultValue.number)]];
    inner.forEach((item, index) => {
        fields.push([entry.batch.accounts, String(item.accounts.length)]);
        fields.push(...item.fields);
    });
    return {
        program: root.program.publicKey,
        instruction: instruction.name,
        data: hex(bytes),
        accounts: inner.flatMap((item) => item.accounts),
        inner: inner.map((item) => item.instruction),
        fields,
    };
}

function vector(entry, root, instruction, seed, types) {
    if (entry.batch?.instruction === instruction.name) return batchVector(entry, root, instruction, seed, types);
    const path = [root, root.program, instruction];
    const bytes = instructionBytes(path, instruction, seed, types);
    const parsed = parseInstructionData(root, bytes);
    if (!parsed) throw new Error(`对照工具认不出 ${instruction.name}`);
    const found = parsed.path[parsed.path.length - 1];
    const fields = [];
    flattenObject(parsed.data, "", fields);
    return {
        program: root.program.publicKey,
        instruction: found.name,
        data: hex(bytes),
        accounts: (instruction.accounts ?? [])
            .filter((account) => account.isOptional !== true)
            .map((account) => account.name),
        fields,
    };
}

function writeVectors(schemas) {
    const instructions = [];
    let seed = 0;
    for (const entry of schemas) {
        const root = createFromJson(JSON.stringify(entry.root)).getRoot();
        const types = Object.fromEntries((root.program.definedTypes ?? []).map((item) => [item.name, item.type]));
        for (const instruction of root.program.instructions) {
            seed += 1;
            instructions.push(vector(entry, root, instruction, seed, types));
        }
    }
    const text = `${JSON.stringify({ generator: "tools/crosscheck/codama/generate.mjs", instructions }, null, 1)}\n`;
    writeFileSync(VECTORS, text);
    const lock = readFileSync(VECTORS_LOCK, "utf8")
        .split("\n")
        .filter((line) => line.trim() && !line.endsWith(` ${VECTORS_NAME}`));
    writeFileSync(VECTORS_LOCK, `${[...lock, `${sha256(Buffer.from(text))} ${VECTORS_NAME}`].join("\n")}\n`);
    return instructions.length;
}

const schemas = loadSchemas();
const asset = writeAsset(schemas);
const count = writeVectors(schemas);
console.log(`${asset.programs.length} 个程序,${count} 个向量`);
