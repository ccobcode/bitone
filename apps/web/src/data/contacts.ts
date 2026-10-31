// 地址簿。名称和历史都不是可信依据，地址都经过核心库校验。
import { primaryNetwork, validateAddress, type Chain, type Mode } from "../core";
import { addressKey } from "../gateway/client";
import { cleanText, lookalike } from "../core/text";
import { db, type Contact, type TxRecord } from "./db";
import { t } from "../core/i18n";

export const MAX_CONTACT_NAME = 32;

export class ContactError extends Error {}

export interface ContactInput {
    id?: string;
    name: string;
    chain: Chain;
    mode: Mode;
    address: string;
}

/** 校验并返回规范写法。地址不属于这条链时抛错。 */
export function canonicalAddress(chain: Chain, mode: Mode, address: string): string {
    return validateAddress(primaryNetwork(chain, mode), address);
}

export async function saveContact(input: ContactInput): Promise<Contact> {
    const name = cleanText(input.name.trim(), MAX_CONTACT_NAME);
    if (!name) throw new ContactError(t("请填写名称。"));
    let address: string;
    try {
        address = canonicalAddress(input.chain, input.mode, input.address);
    } catch {
        throw new ContactError(t("地址格式错误。"));
    }
    return db.transaction("rw", db.contacts, async () => {
        const rows = await db.contacts.where({ chain: input.chain, mode: input.mode }).toArray();
        const same = rows.find((row) => addressKey(row.address) === addressKey(address) && row.id !== input.id);
        if (same) throw new ContactError(t("地址已在地址簿：{0}。", same.name));
        const existing = input.id ? await db.contacts.get(input.id) : undefined;
        const contact: Contact = {
            id: existing?.id ?? crypto.randomUUID(),
            name,
            chain: input.chain,
            mode: input.mode,
            address,
            // 编辑不改变确认状态，待确认的联系人要走确认这一步。
            pending: existing?.pending ?? false,
            createdAt: existing?.createdAt ?? Date.now(),
        };
        await db.contacts.put(contact);
        return contact;
    });
}

export async function confirmContact(id: string): Promise<void> {
    await db.contacts.update(id, { pending: false });
}

export async function removeContact(id: string): Promise<void> {
    await db.contacts.delete(id);
}

/** 与地址簿里已确认的地址首尾相同但中间不同的联系人。保存和确认时都要提示。 */
export function similarContact(address: string, contacts: readonly Contact[]): Contact | null {
    return usableContacts(contacts).find((contact) => lookalike(address, contact.address)) ?? null;
}

/** 待确认的联系人不能用于转账。 */
export function usableContacts(contacts: readonly Contact[]): Contact[] {
    return contacts.filter((contact) => !contact.pending);
}

export interface RecipientCheck {
    /** 地址簿里已确认的联系人。 */
    contact: Contact | null;
    /** 地址在地址簿里，但还没有确认。 */
    pending: Contact | null;
    /** 这个钱包没有成功转给过这个地址。 */
    firstTime: boolean;
    /** 首尾相同但中间不同的已有地址。 */
    similar: { name: string; address: string } | null;
}

/**
 * 判断收款地址，见 docs/04-security.md 第 4 节。
 * 只看本地构建并确认上链的转账，不看链上历史，投毒者能伪造零金额的转出记录。
 */
export function checkRecipient(
    address: string,
    contacts: readonly Contact[],
    txs: readonly Pick<TxRecord, "to" | "state">[],
    own: readonly string[],
): RecipientCheck {
    const key = addressKey(address);
    const same = contacts.find((contact) => addressKey(contact.address) === key) ?? null;
    const paid = txs.filter((tx) => tx.state === "confirmed").map((tx) => tx.to);
    const known = [
        ...usableContacts(contacts).map((contact) => ({ name: `「${contact.name}」`, address: contact.address })),
        ...own.map((item) => ({ name: t("本钱包地址"), address: item })),
        ...paid.map((item) => ({ name: t("转账记录"), address: item })),
    ];
    return {
        contact: same && !same.pending ? same : null,
        pending: same?.pending ? same : null,
        firstTime: !paid.some((item) => addressKey(item) === key),
        similar: known.find((item) => lookalike(address, item.address)) ?? null,
    };
}
