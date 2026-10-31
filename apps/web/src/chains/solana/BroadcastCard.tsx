import { useState } from "react";
import { t } from "../../core/i18n";
import { planOf } from "../../data/db";
import { ActionBlock } from "../../ui/ActionBlock";
import { Button, Helper, Notice } from "../../ui/primitives";
import type { BroadcastCardProps } from "../spec";

/** Solana 不提供加速.节点丢弃时可以重发同一笔,推进 nonce 可以作废它. */
export function BroadcastCard({ tx, busy, failure, replace, resend }: BroadcastCardProps) {
    const [notice, setNotice] = useState("");

    async function send() {
        setNotice("");
        try {
            await resend();
            setNotice(t("已重新提交."));
        } catch {
            setNotice(t("提交失败,请稍后重试."));
        }
    }

    return (
        <ActionBlock
            id="h-act"
            title={t("不能加速")}
            helper={
                <>
                    <Helper>{t("Solana 不提供加速,推进 nonce 可以作废这笔交易.")}</Helper>
                    {notice ? (
                        <p role="status" className="text-t2 text-text-2">
                            {notice}
                        </p>
                    ) : null}
                    {failure ? <Notice tone="danger">{failure}</Notice> : null}
                </>
            }
        >
            {planOf(tx, "solana").nonceValue === null ? null : (
                <div className="flex gap-s2">
                    <Button disabled={busy} onClick={() => void replace("cancel")}>
                        {t("推进 nonce")}
                    </Button>
                    <Button variant="primary" onClick={() => void send()}>
                        {t("重新广播")}
                    </Button>
                </div>
            )}
        </ActionBlock>
    );
}
