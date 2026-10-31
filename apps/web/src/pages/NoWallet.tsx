import { useApp } from "../data/app";
import { modeLabel } from "../ui/labels";
import { Empty, LinkButton } from "../ui/primitives";
import { overlayLink } from "../ui/Sheet";
import { t } from "../core/i18n";

/** 另一个网络已有钱包，当前网络还没有。 */
export function NoWallet() {
    const { mode } = useApp();
    return (
        <Empty
            icon="wallet"
            action={
                <LinkButton variant="primary" {...overlayLink("?connect=1")}>
                    {t("配对新钱包")}
                </LinkButton>
            }
        >
            {t("这个浏览器还没有")}
            {modeLabel(mode)}
            {t("钱包。")}
        </Empty>
    );
}
