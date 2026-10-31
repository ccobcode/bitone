import { t } from "../../core/i18n";
import { ActionBlock } from "../../ui/ActionBlock";
import { Helper } from "../../ui/primitives";

/** TRON 不提供加速和取消. */
export function BroadcastCard() {
    return (
        <ActionBlock
            id="h-act"
            title={t("不能加速")}
            helper={<Helper>{t("TRON 不提供加速,超过过期时间后这笔交易自动失效.")}</Helper>}
        />
    );
}
