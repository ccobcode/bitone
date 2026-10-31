import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { langChoice, setLang, t, translate } from "./i18n";

afterEach(() => {
    vi.restoreAllMocks();
    setLang("zh");
});

describe("setLang", () => {
    beforeEach(() => {
        const saved = new Map<string, string>();
        vi.stubGlobal("localStorage", {
            getItem: (key: string) => saved.get(key) ?? null,
            setItem: (key: string, value: string) => saved.set(key, value),
            removeItem: (key: string) => saved.delete(key),
        });
    });

    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it("自动跟随系统语言,不保存具体语言", () => {
        vi.spyOn(navigator, "language", "get").mockReturnValue("en-US");
        setLang("zh");
        setLang("auto");
        expect(langChoice()).toBe("auto");
        expect(t("收款")).toBe("Receive");

        vi.spyOn(navigator, "language", "get").mockReturnValue("zh-TW");
        setLang("auto");
        expect(t("收款")).toBe("收款");
    });

    it("没有对应语言的系统用英文", () => {
        vi.spyOn(navigator, "language", "get").mockReturnValue("ja-JP");
        setLang("auto");
        expect(t("收款")).toBe("Receive");
    });

    it("选过之后以选择为准", () => {
        vi.spyOn(navigator, "language", "get").mockReturnValue("en-US");
        setLang("zh");
        expect(langChoice()).toBe("zh");
        expect(t("收款")).toBe("收款");
    });
});

describe("t", () => {
    it("中文时原样返回并填入参数", () => {
        expect(t("至少 {0} 个字符", 12)).toBe("至少 12 个字符");
    });

    it("英文时用词典", () => {
        setLang("en");
        expect(t("收款")).toBe("Receive");
        expect(t("至少 {0} 个字符", 12)).toBe("At least 12 characters");
    });

    it("没有译文时显示原文", () => {
        setLang("en");
        expect(t("这句没有译文")).toBe("这句没有译文");
    });
});

describe("translate", () => {
    it("核心库的文案按模板翻译,动态内容原样保留", () => {
        setLang("en");
        expect(translate("普通转账")).toBe("Transfer");
        expect(translate("对方可随时转走你全部的 USDT.")).toBe("The recipient can move all your USDT at any time.");
    });

    it("金额和地址不处理", () => {
        setLang("en");
        expect(translate("1.5 ETH")).toBe("1.5 ETH");
        expect(translate("0xdAC17F958D2ee523a2206206994597C13D831ec7")).toBe(
            "0xdAC17F958D2ee523a2206206994597C13D831ec7",
        );
    });

    it("中文时不翻译", () => {
        expect(translate("普通转账")).toBe("普通转账");
    });
});
