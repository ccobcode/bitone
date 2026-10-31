import { t } from "../core/i18n";
/** 接收帧的进度,配对和签名共用. */
export function ScanProgress({ value, frames }: { value: number; frames: number }) {
    const done = Math.round(value * frames);
    return (
        <div className="flex flex-col gap-s2">
            <div className="flex justify-between text-t2">
                <span className="text-text-3">{t("已接收")}</span>
                <span className="text-text-2">{frames ? t("{0}/{1} 帧", done, frames) : t("等待首帧")}</span>
            </div>
            <div aria-hidden="true" className="h-1 overflow-hidden rounded-rf bg-surface-3">
                <div
                    className={`meter-fill h-full rounded-rf ${value >= 1 ? "bg-ok" : "bg-accent"}`}
                    style={{ width: `${value * 100}%` }}
                />
            </div>
        </div>
    );
}
