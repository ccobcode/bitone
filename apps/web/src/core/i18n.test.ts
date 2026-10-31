import { afterEach, describe, expect, it } from "vitest";
import { setLang, t, translate } from "./i18n";

afterEach(() => setLang("zh"));

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
    it("核心库的文案按模板翻译，动态内容原样保留", () => {
        setLang("en");
        expect(translate("普通转账")).toBe("Transfer");
        expect(translate("对方可随时转走你全部的 USDT。")).toBe("The recipient can move all your USDT at any time.");
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
