// 把扫码结果写入 wallets、accounts、addresses。
import { deriveAddress, isLegacyKind, primaryNetwork, type Mode, type Pairing, type PairingAccount } from "../core";
import { db, type Account, type Address, type Wallet } from "./db";
import { t } from "../core/i18n";

/** 接收和找零各派生到这里，对应 gap limit。 */
export const GAP_LIMIT = 20;

/** 与核心库的 MAX_ADDRESS_INDEX 一致。 */
export const MAX_ADDRESS_INDEX = 100_000;

export function walletId(fingerprint: string, mode: Mode): string {
    return `${fingerprint}:${mode}`;
}

export function addressRows(account: Account, mode: Mode): Address[] {
    if (account.chain !== "bitcoin") return [newAddress(account, mode, 0, 0)];
    const rows: Address[] = [];
    for (let index = 0; index < GAP_LIMIT; index++) {
        rows.push(newAddress(account, mode, index, 0), newAddress(account, mode, index, 1));
    }
    return rows;
}

export interface PairingChoice {
    name: string;
    /** 勾选的账户路径，两个模式合在一起。旧路径账户不显示给用户，总是一起保存。 */
    paths: Set<string>;
    /** 用户当前的网络模式，返回这个模式的钱包。 */
    mode: Mode;
}

const MODES: Mode[] = ["mainnet", "testnet"];

function belongsTo(account: PairingAccount, mode: Mode): boolean {
    return account.scope === "both" || account.scope === mode;
}

/** 用户能选的账户。传了 mode 时只列这个网络模式下的。 */
export function selectableAccounts(pairing: Pairing, mode?: Mode): PairingAccount[] {
    return pairing.accounts.filter(
        (account) => !isLegacyKind(account.kind) && (mode === undefined || belongsTo(account, mode)),
    );
}

/** 一次扫码写入主网和测试网两个观察钱包。已有的钱包和账户保持原样，只补缺的账户。 */
export async function savePairing(pairing: Pairing, { name, paths, mode }: PairingChoice): Promise<Wallet> {
    const plans = MODES.map((target) => {
        const id = walletId(pairing.fingerprint, target);
        const wallet: Wallet = { id, fingerprint: pairing.fingerprint, name, mode: target, pairedAt: Date.now() };
        const accounts: Account[] = pairing.accounts
            .filter((account) => belongsTo(account, target))
            .filter((account) => paths.has(account.path) || isLegacyKind(account.kind))
            .map(({ chain, kind, path, key }) => ({
                id: `${id}:${path}`,
                walletId: id,
                chain,
                kind,
                path,
                key,
                network: primaryNetwork(chain, target),
            }));
        // 地址要在事务外算好。派生失败时什么都不写。
        return { wallet, rows: accounts.map((account) => ({ account, addresses: addressRows(account, target) })) };
    }).filter((plan) => plan.rows.length > 0);

    const saved = await db.transaction("rw", db.wallets, db.accounts, db.addresses, db.settings, async () => {
        const wallets: Wallet[] = [];
        for (const { wallet, rows } of plans) {
            const existing = await db.wallets.get(wallet.id);
            if (!existing) await db.wallets.add(wallet);
            wallets.push(existing ?? wallet);
            // 切换网络时留在同一个冷钱包上。
            await db.settings.put({ key: `wallet:${wallet.mode}`, value: wallet.id });
            for (const { account, addresses } of rows) {
                if (await db.accounts.get(account.id)) continue;
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
    if (index >= MAX_ADDRESS_INDEX) throw new Error(t("地址序号超出上限"));
    return {
        accountId: account.id,
        change,
        index,
        address: deriveAddress(account, mode, change, index),
        verified: false,
    };
}

/** 取得一个地址，还没派生时派生，不写入。 */
export async function draftAddress(account: Account, mode: Mode, index: number, change: 0 | 1): Promise<Address> {
    return (await db.addresses.get([account.id, change, index])) ?? newAddress(account, mode, index, change);
}

/** 取得一个地址，还没派生时派生并写入。 */
export async function ensureAddress(account: Account, mode: Mode, index: number, change: 0 | 1 = 0): Promise<Address> {
    const existing = await db.addresses.get([account.id, change, index]);
    if (existing) return existing;
    const address = newAddress(account, mode, index, change);
    await db.addresses.put(address);
    return address;
}

/** 账户的第一个收款地址，账户的核对状态看的是它。 */
export function firstAddress(addresses: readonly Address[], accountId: string): Address | undefined {
    return addresses.find((a) => a.accountId === accountId && a.change === 0 && a.index === 0);
}

export function addressPath(account: Account, address: Pick<Address, "change" | "index">): string {
    return account.chain === "bitcoin" ? `${account.path}/${address.change}/${address.index}` : account.path;
}

/** 首次写入后申请持久化。被拒绝时不阻断，W6 的本地数据区显示未持久化。 */
export async function requestPersist(): Promise<boolean> {
    return (await navigator.storage?.persist?.().catch(() => false)) ?? false;
}

export async function isPersisted(): Promise<boolean> {
    return (await navigator.storage?.persisted?.().catch(() => false)) ?? false;
}
