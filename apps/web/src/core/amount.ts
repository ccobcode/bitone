// 金额只用 BigInt。网关的金额是最小单位的整数字符串。

/** BigInt('0x10')、BigInt('') 都不报错，所以先按十进制整数校验。 */
export function parseAmount(value: unknown): bigint | null {
    if (typeof value !== "string" || !/^\d{1,78}$/.test(value)) return null;
    return BigInt(value);
}

export interface FormatOptions {
    /** 至少显示的小数位。 */
    min?: number;
    /** 最多显示的小数位，多出的部分舍去，不进位。 */
    max?: number;
}

export function formatUnits(raw: bigint, decimals: number, { min = 0, max = decimals }: FormatOptions = {}): string {
    const base = 10n ** BigInt(decimals);
    const integer = raw / base;
    const fraction = (raw % base).toString().padStart(decimals, "0");
    const shown = fraction.slice(0, max);
    const trimmed = shown.replace(/0+$/, "").padEnd(Math.min(min, shown.length), "0");
    const whole = integer.toLocaleString("en-US");
    const text = trimmed ? `${whole}.${trimmed}` : whole;
    if (raw > 0n && integer === 0n && !/[1-9]/.test(shown)) {
        return `<0.${"0".repeat(Math.max(max - 1, 0))}1`;
    }
    return text;
}

/** 用户输入的十进制金额转成最小单位。格式不对或小数位超出时返回 null，不用浮点数。 */
export function parseUnits(text: string, decimals: number): bigint | null {
    const value = text.trim().replaceAll(",", "");
    const match = /^(\d+)(?:\.(\d*))?$/.exec(value);
    if (!match || value.length > 78) return null;
    const fraction = match[2] ?? "";
    if (fraction.length > decimals) return null;
    return BigInt(match[1] ?? "0") * 10n ** BigInt(decimals) + BigInt(fraction.padEnd(decimals, "0") || "0");
}

/** 最小单位转回输入框里的文字，不分组。 */
export function toInput(raw: bigint, decimals: number): string {
    return formatUnits(raw, decimals).replaceAll(",", "");
}

/** 美元金额，单位是分。价格是网关给的十进制字符串，只用于展示。 */
export function usdCents(amount: bigint, decimals: number, price: string): bigint | null {
    const match = /^(\d{1,30})(?:\.(\d{1,30}))?$/.exec(price);
    if (!match) return null;
    const fraction = match[2] ?? "";
    const scaled = BigInt(`${match[1]}${fraction}`);
    return (amount * scaled * 100n) / 10n ** BigInt(decimals + fraction.length);
}

export function formatUsd(cents: bigint): string {
    return `$${formatUnits(cents, 2, { min: 2, max: 2 })}`;
}
