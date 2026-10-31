import type { ReactNode } from "react";
import { Section, SectionTitle } from "./primitives";

/** 需要用户决定的一件事:左边说明,右边按钮. */
export function ActionBlock({
    id,
    title,
    helper,
    tone,
    children,
}: {
    id: string;
    title: string;
    helper: ReactNode;
    tone?: string;
    children?: ReactNode;
}) {
    return (
        <Section aria-labelledby={id} className="rise flex-row flex-wrap items-center justify-end gap-s4">
            <div className="flex min-w-0 flex-[1_1_320px] flex-col gap-s1">
                <SectionTitle id={id} className={tone}>
                    {title}
                </SectionTitle>
                {helper}
            </div>
            {children}
        </Section>
    );
}
