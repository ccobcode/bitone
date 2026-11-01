// 地址行的读写.怎么派生由调用方传入,这里不区分链.
import type { Mode } from "../core";
import { t } from "../core/i18n";
import { db, type Account, type Address } from "./db";

/** BIP-32 普通索引的上限,再往上是硬化索引,公钥派生不出来. */
export const MAX_ADDRESS_INDEX = 0x8000_0000;

export type DeriveAddress = (
    account: Pick<Account, "chain" | "kind" | "key">,
    mode: Mode,
    change: 0 | 1,
    index: number,
) => string;

export function newAddress(derive: DeriveAddress, account: Account, mode: Mode, index: number, change: 0 | 1): Address {
    if (index >= MAX_ADDRESS_INDEX) throw new Error(t("地址序号超出上限"));
    return {
        accountId: account.id,
        change,
        index,
        address: derive(account, mode, change, index),
        verified: false,
    };
}

/** 取得一个地址,还没派生时派生并写入. */
export async function storeAddress(
    derive: DeriveAddress,
    account: Account,
    mode: Mode,
    index: number,
    change: 0 | 1,
): Promise<Address> {
    const existing = await db.addresses.get([account.id, change, index]);
    if (existing) return existing;
    const address = newAddress(derive, account, mode, index, change);
    await db.addresses.put(address);
    return address;
}
