// 来自链上或网关的文本只当纯文本,这里再去掉控制字符和不可见字符.
export function cleanText(value: string, max = 64): string {
    return value.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, "").slice(0, max);
}

/** EVM 地址的大小写只是校验和,统一转小写比较.Base58 区分大小写,不能转换. */
export function addressKey(address: string): string {
    return address.startsWith("0x") ? address.toLowerCase() : address;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null;
}

/** 列表里的短格式,如 0x7a3F…9C21. */
export function shortAddress(address: string): string {
    const head = address.startsWith("0x") || address.startsWith("bc1") || address.startsWith("tb1") ? 6 : 4;
    if (address.length <= head + 5) return address;
    return `${address.slice(0, head)}…${address.slice(-4)}`;
}

/** 需要核对的地方显示完整地址,4 个字符一组. */
export function groupAddress(address: string): { offset: number; text: string }[] {
    // 分段和冷钱包一样,0x 连着前四位,两边才能逐段对比.
    const head = address.startsWith("0x") ? 6 : 4;
    const rest = [...address.slice(head).matchAll(/.{1,4}/g)];
    return [
        { offset: 0, text: address.slice(0, head) },
        ...rest.map((match) => ({ offset: head + match.index, text: match[0] })),
    ];
}

/**
 * 首尾相同但中间不同,这是地址投毒的特征.
 * 比较的就是列表里显示的短格式,看起来一样才危险.
 * 十六进制地址不看大小写,大小写不同的同一个地址不算相似.
 */
export function lookalike(a: string, b: string): boolean {
    const [left, right] = [addressKey(a), addressKey(b)];
    return left !== right && shortAddress(left) === shortAddress(right);
}
