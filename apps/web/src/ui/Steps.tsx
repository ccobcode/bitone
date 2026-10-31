import { Icon } from "./Icon";
import { t } from "../core/i18n";

const LABELS = [t("填写"), t("签名"), t("广播")];
const DOTS = {
    on: "bg-accent text-on-accent",
    live: "step-live bg-accent text-on-accent",
    done: "bg-accent-soft text-accent-text",
    finished: "ring-once bg-ok-soft text-ok",
    failed: "ring-once bg-danger-soft text-danger",
    todo: "bg-surface-3 text-text-3",
};
const TEXTS = {
    on: "font-medium text-text",
    live: "font-medium text-text",
    done: "text-text-2",
    finished: "font-medium text-ok",
    failed: "font-medium text-danger",
    todo: "text-text-3",
};

/** 第三步的结果。广播之后这一步还要等链上确认。 */
export type StepResult = "live" | "finished" | "failed";

/** 转账步骤，放在页头右侧，不单独占一行。手机上只有圆点，步骤名就是页面标题，和冷钱包相同。切页时外壳不重新挂载，连线是真的填上去的。 */
export function Steps({ current, result }: { current: 1 | 2 | 3; result?: StepResult }) {
    return (
        <ol aria-label={t("转账步骤")} className="flex shrink-0 items-center gap-s2">
            {LABELS.map((label, i) => {
                const step = i + 1;
                const phase = step === current ? (result ?? "on") : step < current ? "done" : "todo";
                const active = step === current;
                return (
                    <li key={label} aria-current={active ? "step" : undefined} className="flex items-center gap-s2">
                        {i > 0 ? (
                            <span
                                aria-hidden="true"
                                data-reached={step <= current ? "" : undefined}
                                className="block h-0.5 w-s4 overflow-hidden rounded-rf bg-surface-3 nav:w-s6"
                            >
                                <span className="step-fill block h-full rounded-rf bg-accent" />
                            </span>
                        ) : null}
                        <span
                            aria-hidden="true"
                            className={`flex size-5 items-center justify-center rounded-full text-t1 font-medium transition-colors ${DOTS[phase]}`}
                        >
                            {phase === "done" || phase === "finished" ? (
                                <Icon name="check" size={12} />
                            ) : phase === "failed" ? (
                                <Icon name="close" size={12} />
                            ) : (
                                step
                            )}
                        </span>
                        <span className={`sr-only text-t2 transition-colors nav:not-sr-only ${TEXTS[phase]}`}>
                            {phase === "finished" ? t("完成") : phase === "failed" ? t("链上失败") : label}
                        </span>
                    </li>
                );
            })}
        </ol>
    );
}
