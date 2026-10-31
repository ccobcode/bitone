import type { ReactNode } from "react";
import { SectionTitle } from "./primitives";
import { Icon, type IconName } from "./Icon";

interface StateCardProps {
    tone: "danger" | "warn";
    icon: IconName;
    title: string;
    body: string;
    facts: { label: string; value: ReactNode }[];
    actions?: ReactNode;
}

const TONES = {
    danger: { block: "bg-danger-soft", text: "text-danger" },
    warn: { block: "bg-warn-soft", text: "text-warn" },
};

/** 异常态,对应原型的 WS-States. */
export function StateCard({ tone, icon, title, body, facts, actions }: StateCardProps) {
    const { block, text } = TONES[tone];
    return (
        <section className={`tint rise flex min-w-0 flex-col gap-s4 rounded-r3 p-s4 @split:p-s6 ${block}`}>
            <div className="flex items-start gap-s3">
                <Icon name={icon} size={24} className={`shrink-0 ${text}`} />
                <div className="flex min-w-0 flex-1 flex-col gap-s1">
                    <SectionTitle className={text}>{title}</SectionTitle>
                    <p className="text-t3 text-text-2">{body}</p>
                </div>
            </div>
            {facts.length > 0 ? (
                <dl className="flex flex-wrap gap-x-s8 gap-y-s3 pl-s8">
                    {facts.map((fact) => (
                        <div key={fact.label} className="flex min-w-0 flex-col gap-s1">
                            <dt className="text-t2 text-text-3">{fact.label}</dt>
                            <dd className="text-t3 [overflow-wrap:anywhere]">{fact.value}</dd>
                        </div>
                    ))}
                </dl>
            ) : null}
            {actions ? <div className="flex flex-wrap justify-end gap-s2">{actions}</div> : null}
        </section>
    );
}
