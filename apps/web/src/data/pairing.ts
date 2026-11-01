// 把扫码结果写入 wallets,accounts,addresses.
import { chainOf } from "../chains";
import { isLegacyKind, primaryNetwork, type Mode, type Pairing, type PairingAccount } from "../core";
import { MAX_ADDRESS_INDEX, newAddress as deriveRow, storeAddress } from "./addresses";
import { db, type Account, type Address, type FeedCursor, type Wallet } from "./db";
import { t } from "../core/i18n";

/** 接收和找零各派生到这里,对应 gap limit. */
export const GAP_LIMIT = 20;

export { MAX_ADDRESS_INDEX };

export function walletId(fingerprint: string, mode: Mode): string {
    return `${fingerprint}:${mode}`;
}

export function addressRows(account: Account, mode: Mode): Address[] {
    if (!chainOf(account.chain).hdAddresses) return [newAddress(account, mode, 0, 0)];
    const rows: Address[] = [];
    for (let index = 0; index < GAP_LIMIT; index++) {
        rows.push(newAddress(account, mode, index, 0), newAddress(account, mode, index, 1));
    }
    return rows;
}

export interface PairingChoice {
    name: string;
    /** 勾选的账户路径,两个模式合在一起.旧路径账户不显示给用户,总是一起保存. */
    paths: Set<string>;
    /** 用户当前的网络模式,返回这个模式的钱包. */
    mode: Mode;
}

const MODES: Mode[] = ["mainnet", "testnet"];

function belongsTo(account: PairingAccount, mode: Mode): boolean {
    return account.scope === "both" || account.scope === mode;
}

/** 用户能选的账户.传了 mode 时只列这个网络模式下的. */
export function selectableAccounts(pairing: Pairing, mode?: Mode): PairingAccount[] {
    return pairing.accounts.filter(
        (account) => !isLegacyKind(account.kind) && (mode === undefined || belongsTo(account, mode)),
    );
}

export function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
    return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

/** 旧公钥派生的地址和它们的同步记录都作废.新地址都未核对,钱包的历史按新地址重新同步. */
async function replaceKey(account: Account, addresses: Address[]): Promise<void> {
    const rows = db.addresses.where("accountId").equals(account.id);
    const stale = new Set((await rows.toArray()).map((row) => row.address));
    await rows.delete();
    await db.accounts.put(account);
    await db.addresses.bulkAdd(addresses);
    await db.settings.where("key").startsWith(`discovery:${account.id}:`).delete();
    const owned = (cursor: FeedCursor) => cursor.id.startsWith(`${account.walletId}:`);
    const cursors = await db.feedCursors
        .filter((cursor) => owned(cursor) && cursor.addresses.some((address) => stale.has(address)))
        .toArray();
    const streams = new Set(cursors.map((cursor) => cursor.id));
    await db.feedCursors.bulkDelete([...streams]);
    await db.chainTxs
        .where("walletId")
        .equals(account.walletId)
        .filter((row) => streams.has(row.stream))
        .delete();
}

/**
 * 一次扫码写入主网和测试网两个观察钱包.已有的钱包保持原样,只补缺的账户.
 * 公钥以手机为准,本地不同的账户换成手机的公钥,没有勾选的也一样.
 */
export async function savePairing(pairing: Pairing, { name, paths, mode }: PairingChoice): Promise<Wallet> {
    const plans = MODES.map((target) => {
        const id = walletId(pairing.fingerprint, target);
        const wallet: Wallet = { id, fingerprint: pairing.fingerprint, name, mode: target, pairedAt: Date.now() };
        const rows = pairing.accounts
            .filter((account) => belongsTo(account, target))
            .map(({ chain, kind, path, key }) => {
                const account: Account = {
                    id: `${id}:${path}`,
                    walletId: id,
                    chain,
                    kind,
                    path,
                    key,
                    network: primaryNetwork(chain, target),
                };
                // 地址要在事务外算好.派生失败时什么都不写.
                const addresses = addressRows(account, target);
                return { account, addresses, chosen: paths.has(path) || isLegacyKind(kind) };
            });
        return { wallet, rows };
    });

    const tables = [db.wallets, db.accounts, db.addresses, db.settings, db.feedCursors, db.chainTxs];
    const saved = await db.transaction("rw", tables, async () => {
        const wallets: Wallet[] = [];
        for (const { wallet, rows } of plans) {
            if (rows.some((row) => row.chosen)) {
                const existing = await db.wallets.get(wallet.id);
                if (!existing) await db.wallets.add(wallet);
                wallets.push(existing ?? wallet);
                // 切换网络时留在同一个冷钱包上.
                await db.settings.put({ key: `wallet:${wallet.mode}`, value: wallet.id });
            }
            for (const { account, addresses, chosen } of rows) {
                const stored = await db.accounts.get(account.id);
                if (stored && !sameBytes(stored.key, account.key)) await replaceKey(account, addresses);
                if (stored || !chosen) continue;
                await db.accounts.add(account);
                await db.addresses.bulkAdd(addresses);
            }
        }
        return wallets;
    });
    const wallet = saved.find((item) => item.mode === mode);
    if (!wallet) throw new Error(t("没有可写入的账户"));
    return wallet;
}

function newAddress(account: Account, mode: Mode, index: number, change: 0 | 1): Address {
    return deriveRow(chainOf(account.chain).deriveAddress, account, mode, index, change);
}

/** 取得一个地址,还没派生时派生,不写入. */
export async function draftAddress(account: Account, mode: Mode, index: number, change: 0 | 1): Promise<Address> {
    return (await db.addresses.get([account.id, change, index])) ?? newAddress(account, mode, index, change);
}

/** 取得一个地址,还没派生时派生并写入. */
export function ensureAddress(account: Account, mode: Mode, index: number, change: 0 | 1 = 0): Promise<Address> {
    return storeAddress(chainOf(account.chain).deriveAddress, account, mode, index, change);
}

/** 账户的第一个收款地址,账户的核对状态看的是它. */
export function firstAddress(addresses: readonly Address[], accountId: string): Address | undefined {
    return addresses.find((a) => a.accountId === accountId && a.change === 0 && a.index === 0);
}

export function addressPath(account: Account, address: Pick<Address, "change" | "index">): string {
    return chainOf(account.chain).hdAddresses ? `${account.path}/${address.change}/${address.index}` : account.path;
}

/** 首次写入后申请持久化.被拒绝时不阻断,W6 的本地数据区显示未持久化. */
export async function requestPersist(): Promise<boolean> {
    return (await navigator.storage?.persist?.().catch(() => false)) ?? false;
}

export async function isPersisted(): Promise<boolean> {
    return (await navigator.storage?.persisted?.().catch(() => false)) ?? false;
}
