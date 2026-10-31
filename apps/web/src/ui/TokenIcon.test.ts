// 图标标识来自内置清单和各链注册表,两边在运行时才汇合,编译器查不到漏注册,这里逐项确认.
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeAll, describe, expect, it } from "vitest";
import { NETWORK_FAMILIES } from "../chains";
import { networkList } from "../core";
import { assetsOf, lookupAsset } from "../core/manifest";
import { initCore } from "../testing";
import { TokenIcon } from "./TokenIcon";

beforeAll(initCore);

describe("图标", () => {
    it("内置清单的每个资产都有图标", () => {
        for (const { id } of networkList()) {
            for (const asset of assetsOf(id)) {
                const entry = lookupAsset(id, asset);
                expect(entry, `${id} ${asset}`).not.toBeNull();
                const html = renderToStaticMarkup(createElement(TokenIcon, { asset: entry, size: 20 }));
                expect(html, `${id} ${asset}`).toContain("<img");
            }
        }
    });

    it("每个网络都属于一组网络图标", () => {
        for (const { id } of networkList()) {
            expect(
                NETWORK_FAMILIES.some((family) => family.matches(id)),
                id,
            ).toBe(true);
        }
    });
});
