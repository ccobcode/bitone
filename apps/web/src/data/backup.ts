// 加密备份，见 docs/03-system.md 第 3 节。里面有钱包、账户公钥、地址簿和备注。
// 不含网关地址和交易。交易可以从链上重新拉取，签名数据没有必要离开这个浏览器。
//
// 备份可能是攻击者给的，文件里的内容都不可信：
// 地址从公钥重新派生，路径只接受白名单，导入的账户全部未核对，联系人全部待确认。
// 已配对的钱包不接受备份改动账户：同一路径公钥不同时保留本地的，本地没有的路径也不写入，
// 指纹是公开的 4 字节，否则备份能往已配对的钱包里塞进攻击者的公钥。只能由冷钱包重新配对添加。
import {
    accountSpecList,
    deriveAddress,
    primaryNetwork,
    validateAddress,
    type AccountKind,
    type Chain,
    type Mode,
} from "../core";
import { cleanText, isRecord } from "../core/text";
import { addressKey } from "../gateway/client";
import { MAX_CONTACT_NAME } from "./contacts";
import { db, type Account, type Wallet } from "./db";
import { addressRows, walletId } from "./pairing";
import { t } from "../core/i18n";

export const BACKUP_FORMAT = "bitone-backup";
export const BACKUP_VERSION = 1;
export const KDF_ITERATIONS = 600_000;
export const MIN_PASSPHRASE = 12;

const SALT_BYTES = 16;
const IV_BYTES = 12;
const MAX_FILE_BYTES = 8 * 1024 * 1024;
const MAX_NOTE = 500;

export class BackupError extends Error {}

/** 按字符计算，一个表情符号算一个。 */
export function passphraseLongEnough(passphrase: string): boolean {
    return [...passphrase].length >= MIN_PASSPHRASE;
}

function toBase64(bytes: Uint8Array): string {
    let text = "";
    for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(text);
}

function fromBase64(text: string): Uint8Array<ArrayBuffer> {
    return Uint8Array.from(atob(text), (char) => char.charCodeAt(0));
}

async function deriveKey(passphrase: string, salt: Uint8Array<ArrayBuffer>): Promise<CryptoKey> {
    // 同一口令在不同输入法下可能是不同的 Unicode 序列。
    const material = await crypto.subtle.importKey(
        "raw",
        new TextEncoder().encode(passphrase.normalize("NFKC")),
        "PBKDF2",
        false,
        ["deriveKey"],
    );
    return crypto.subtle.deriveKey(
        { name: "PBKDF2", hash: "SHA-256", salt, iterations: KDF_ITERATIONS },
        material,
        { name: "AES-GCM", length: 256 },
        false,
        ["encrypt", "decrypt"],
    );
}

interface BackupFile {
    format: string;
    version: number;
    iterations: number;
    salt: string;
    iv: string;
    data: string;
}

/** 文件头也进认证，改动版本和迭代次数都解不开。 */
function headerBytes(file: Omit<BackupFile, "data">): Uint8Array<ArrayBuffer> {
    return new TextEncoder().encode(`${file.format}\n${file.version}\n${file.iterations}\n${file.salt}\n${file.iv}`);
}

interface BackupPayload {
    wallets: { fingerprint: string; name: string; mode: Mode; pairedAt: number }[];
    accounts: { fingerprint: string; mode: Mode; chain: Chain; kind: AccountKind; path: string; key: string }[];
    contacts: { name: string; chain: Chain; mode: Mode; address: string }[];
    notes: { id: string; text: string; updatedAt: number }[];
}

async function collect(): Promise<BackupPayload> {
    const [wallets, accounts, contacts, notes] = await Promise.all([
        db.wallets.toArray(),
        db.accounts.toArray(),
        db.contacts.toArray(),
        db.notes.toArray(),
    ]);
    const owners = new Map(wallets.map((wallet) => [wallet.id, wallet]));
    return {
        wallets: wallets.map(({ fingerprint, name, mode, pairedAt }) => ({ fingerprint, name, mode, pairedAt })),
        accounts: accounts.flatMap((account) => {
            const owner = owners.get(account.walletId);
            if (!owner) return [];
            const { chain, kind, path, key } = account;
            return [{ fingerprint: owner.fingerprint, mode: owner.mode, chain, kind, path, key: toBase64(key) }];
        }),
        contacts: contacts.map(({ name, chain, mode, address }) => ({ name, chain, mode, address })),
        notes,
    };
}

export async function exportBackup(passphrase: string): Promise<string> {
    if (!passphraseLongEnough(passphrase)) throw new BackupError(t("口令至少 {0} 个字符。", MIN_PASSPHRASE));
    const salt = crypto.getRandomValues(new Uint8Array(SALT_BYTES));
    const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
    const head = {
        format: BACKUP_FORMAT,
        version: BACKUP_VERSION,
        iterations: KDF_ITERATIONS,
        salt: toBase64(salt),
        iv: toBase64(iv),
    };
    const key = await deriveKey(passphrase, salt);
    const plain = new TextEncoder().encode(JSON.stringify(await collect()));
    const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: headerBytes(head) }, key, plain);
    const file: BackupFile = { ...head, data: toBase64(new Uint8Array(sealed)) };
    return JSON.stringify(file);
}

function parseFile(text: string): BackupFile {
    if (text.length > MAX_FILE_BYTES) throw new BackupError(t("文件太大，不是备份文件。"));
    let value: unknown;
    try {
        value = JSON.parse(text);
    } catch {
        throw new BackupError(t("这不是备份文件。"));
    }
    if (!isRecord(value) || value.format !== BACKUP_FORMAT) throw new BackupError(t("这不是备份文件。"));
    if (value.version !== BACKUP_VERSION) throw new BackupError(t("备份文件的版本不受支持。"));
    // 迭代次数是文件给的，不能照用，否则一个构造的文件就能卡死页面。
    if (value.iterations !== KDF_ITERATIONS) throw new BackupError(t("备份文件的参数不受支持。"));
    const { salt, iv, data } = value;
    if (typeof salt !== "string" || typeof iv !== "string" || typeof data !== "string") {
        throw new BackupError(t("这不是备份文件。"));
    }
    return { format: BACKUP_FORMAT, version: BACKUP_VERSION, iterations: KDF_ITERATIONS, salt, iv, data };
}

async function decrypt(file: BackupFile, passphrase: string): Promise<unknown> {
    try {
        const key = await deriveKey(passphrase, fromBase64(file.salt));
        const plain = await crypto.subtle.decrypt(
            { name: "AES-GCM", iv: fromBase64(file.iv), additionalData: headerBytes(file) },
            key,
            fromBase64(file.data),
        );
        return JSON.parse(new TextDecoder().decode(plain));
    } catch {
        throw new BackupError(t("口令错误或文件已损坏。"));
    }
}

const invalid = () => new BackupError(t("备份内容无效，没有导入任何数据。"));

function isMode(value: unknown): value is Mode {
    return value === "mainnet" || value === "testnet";
}

function isChain(value: unknown): value is Chain {
    return value === "bitcoin" || value === "evm" || value === "tron" || value === "solana";
}

function checkedWallets(value: unknown): BackupPayload["wallets"] {
    if (!Array.isArray(value)) throw invalid();
    return value.map((item: unknown) => {
        if (!isRecord(item) || !isMode(item.mode)) throw invalid();
        const { fingerprint, name, pairedAt } = item;
        if (typeof fingerprint !== "string" || !/^[0-9A-F]{8}$/.test(fingerprint)) throw invalid();
        return {
            fingerprint,
            mode: item.mode,
            name: (typeof name === "string" && cleanText(name.trim(), 32)) || t("主钱包"),
            pairedAt: typeof pairedAt === "number" ? pairedAt : Date.now(),
        };
    });
}

/** 只接受这个模式下的路径。旧路径账户也在这张表里。 */
function checkedAccounts(value: unknown): (BackupPayload["accounts"][number] & { bytes: Uint8Array })[] {
    if (!Array.isArray(value)) throw invalid();
    return value.map((item: unknown) => {
        if (!isRecord(item) || !isMode(item.mode)) throw invalid();
        const { fingerprint, path, key } = item;
        if (typeof fingerprint !== "string" || typeof path !== "string" || typeof key !== "string") throw invalid();
        const spec = accountSpecList(item.mode).find((row) => row.path === path);
        if (!spec || spec.chain !== item.chain || spec.kind !== item.kind) throw invalid();
        const bytes = fromBase64(key);
        try {
            deriveAddress({ chain: spec.chain, kind: spec.kind, key: bytes }, item.mode, 0, 0);
        } catch {
            throw invalid();
        }
        return { fingerprint, mode: item.mode, chain: spec.chain, kind: spec.kind, path, key, bytes };
    });
}

function checkedContacts(value: unknown): BackupPayload["contacts"] {
    if (!Array.isArray(value)) throw invalid();
    return value.flatMap((item: unknown) => {
        if (!isRecord(item) || !isChain(item.chain) || !isMode(item.mode) || typeof item.address !== "string")
            throw invalid();
        const name = typeof item.name === "string" ? cleanText(item.name.trim(), MAX_CONTACT_NAME) : "";
        let address: string;
        try {
            address = validateAddress(primaryNetwork(item.chain, item.mode), item.address);
        } catch {
            return [];
        }
        return name ? [{ name, chain: item.chain, mode: item.mode, address }] : [];
    });
}

function checkedNotes(value: unknown): BackupPayload["notes"] {
    if (!Array.isArray(value)) throw invalid();
    return value.flatMap((item: unknown) => {
        if (!isRecord(item) || typeof item.id !== "string" || typeof item.text !== "string") return [];
        const text = cleanText(item.text, MAX_NOTE);
        const updatedAt = typeof item.updatedAt === "number" ? item.updatedAt : Date.now();
        return text ? [{ id: cleanText(item.id, 200), text, updatedAt }] : [];
    });
}

export interface ImportSummary {
    wallets: number;
    accounts: number;
    /** 已配对的钱包里没有写入的账户。公钥不同，或者本地没有这个路径。 */
    conflicts: number;
    contacts: number;
    notes: number;
    /** 第一个导入的钱包，全新的浏览器导入后选中它。 */
    first: Wallet | null;
}

export async function importBackup(text: string, passphrase: string): Promise<ImportSummary> {
    const payload = await decrypt(parseFile(text), passphrase);
    if (!isRecord(payload)) throw invalid();
    const wallets = checkedWallets(payload.wallets);
    const accounts = checkedAccounts(payload.accounts);
    const contacts = checkedContacts(payload.contacts);
    const notes = checkedNotes(payload.notes);

    const rows = accounts.map((item) => {
        const owner = walletId(item.fingerprint, item.mode);
        const account: Account = {
            id: `${owner}:${item.path}`,
            walletId: owner,
            chain: item.chain,
            kind: item.kind,
            network: primaryNetwork(item.chain, item.mode),
            path: item.path,
            key: item.bytes,
        };
        return { account, addresses: addressRows(account, item.mode) };
    });

    return db.transaction("rw", [db.wallets, db.accounts, db.addresses, db.contacts, db.notes], async () => {
        const summary: ImportSummary = { wallets: 0, accounts: 0, conflicts: 0, contacts: 0, notes: 0, first: null };
        const paired = new Set<string>();
        for (const item of wallets) {
            const id = walletId(item.fingerprint, item.mode);
            const existing = await db.wallets.get(id);
            if (existing) {
                paired.add(id);
                summary.first ??= existing;
                continue;
            }
            const wallet: Wallet = { id, ...item };
            await db.wallets.add(wallet);
            summary.wallets += 1;
            summary.first ??= wallet;
        }
        for (const { account, addresses } of rows) {
            if (!(await db.wallets.get(account.walletId))) continue;
            const existing = await db.accounts.get(account.id);
            if (existing ? !sameBytes(existing.key, account.key) : paired.has(account.walletId)) {
                summary.conflicts += 1;
                continue;
            }
            if (!existing) await db.accounts.add(account);
            // 恢复出来的账户全部标为未核对，地址从公钥重新派生。
            await db.addresses.where("accountId").equals(account.id).modify({ verified: false });
            const keys = addresses.map((row): [string, number, number] => [row.accountId, row.change, row.index]);
            const present = await db.addresses.bulkGet(keys);
            await db.addresses.bulkAdd(addresses.filter((_, i) => !present[i]));
            summary.accounts += 1;
        }
        for (const item of contacts) {
            const rows = await db.contacts.where({ chain: item.chain, mode: item.mode }).toArray();
            // 已有的联系人保持原样，备份不能改掉名称或确认状态。
            if (rows.some((row) => addressKey(row.address) === addressKey(item.address))) continue;
            await db.contacts.add({ ...item, id: crypto.randomUUID(), pending: true, createdAt: Date.now() });
            summary.contacts += 1;
        }
        for (const note of notes) {
            if (await db.notes.get(note.id)) continue;
            await db.notes.add(note);
            summary.notes += 1;
        }
        return summary;
    });
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
    return a.length === b.length && a.every((byte, i) => byte === b[i]);
}
