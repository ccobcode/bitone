import { t } from "../core/i18n";
const LABELS = [t("填写"), t("签名"), t("广播")];

/** 转账步骤，W3、W4 共用，第三步是 W4 里核验后的广播。一条细进度，和内容左右对齐。 */
export function Steps({ current }: { current: 1 | 2 | 3 }) {
    return (
        <ol aria-label={t("转账步骤")} className="grid grid-cols-3 gap-s2">
            {LABELS.map((label, i) => {
                const step = i + 1;
                const on = step === current;
                return (
                    <li key={label} className="flex min-w-0 flex-col gap-s2" aria-current={on ? "step" : undefined}>
                        <span
                            aria-hidden="true"
                            data-reached={step <= current ? "" : undefined}
                            className="block h-1 overflow-hidden rounded-rf bg-surface-3"
                        >
                            <span className="step-fill block h-full rounded-rf bg-accent" />
                        </span>
                        <span
                            className={`text-t2 transition-colors ${on ? "font-medium text-text" : step < current ? "text-text-2" : "text-text-3"}`}
                        >
                            {label}
                        </span>
                    </li>
                );
            })}
        </ol>
    );
}
