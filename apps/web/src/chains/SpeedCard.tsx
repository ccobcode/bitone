import { t } from "../core/i18n";
import { ActionBlock } from "../ui/ActionBlock";
import { Button, Helper, Notice } from "../ui/primitives";
import type { BroadcastCardProps } from "./spec";

/** 已广播还没打包时可以加速或取消. */
export function SpeedCard({ busy, failure, overdue, replace }: BroadcastCardProps) {
    return (
        <ActionBlock
            id="h-act"
            title={overdue ? t("等待时间过长") : t("加速或取消")}
            helper={
                <>
                    <Helper tone={overdue ? "text-warn" : "text-text-3"}>
                        {t("两种操作都会生成新的待签名请求,要回到冷钱包签名.")}
                    </Helper>
                    {failure ? <Notice tone="danger">{failure}</Notice> : null}
                </>
            }
        >
            <div className="flex gap-s2">
                <Button disabled={busy} onClick={() => void replace("cancel")}>
                    {t("取消")}
                </Button>
                <Button variant="primary" disabled={busy} onClick={() => void replace("speedup")}>
                    {t("加速")}
                </Button>
            </div>
        </ActionBlock>
    );
}
