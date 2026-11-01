import { useState } from "react";
import { t } from "../core/i18n";
import { Icon } from "./Icon";
import { Button, Collapse, Disclosure, Helper } from "./primitives";

/** 已签名交易的十六进制.只读本地记录,连不上网关也能复制到别处广播. */
export function SignedTx({ raw }: { raw: string }) {
    const [open, setOpen] = useState(false);
    const [copied, setCopied] = useState(false);

    function copy() {
        void navigator.clipboard.writeText(raw).then(
            () => {
                setCopied(true);
                setTimeout(() => setCopied(false), 2000);
            },
            () => undefined,
        );
    }

    return (
        <div className="flex flex-col">
            <Disclosure open={open} onToggle={() => setOpen(!open)}>
                {t("已签名交易")}
            </Disclosure>
            <Collapse open={open}>
                <div className="flex flex-col gap-s2 pt-s2">
                    <Helper>{t("这是十六进制的原始交易,可以在别处广播.拿到它的人都能广播这笔交易.")}</Helper>
                    <p className="max-h-40 overflow-y-auto font-mono text-t2 text-text-2 select-all [overflow-wrap:anywhere]">
                        {raw}
                    </p>
                    <div className="flex justify-end">
                        <Button size="sm" onClick={copy}>
                            <Icon key={String(copied)} name={copied ? "check" : "copy"} size={16} draw={copied} />
                            {copied ? t("已复制") : t("复制十六进制")}
                        </Button>
                    </div>
                </div>
            </Collapse>
        </div>
    );
}
