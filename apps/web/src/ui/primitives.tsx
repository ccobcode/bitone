// 公共小组件。尺寸和颜色都来自令牌。
import {
    createContext,
    use,
    type ButtonHTMLAttributes,
    type ComponentProps,
    type CSSProperties,
    type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { Link, type LinkProps } from "react-router";
import { groupAddress } from "../core/text";
import { Icon, type IconName } from "./Icon";

const BUTTON_BASE =
    "press inline-flex shrink-0 items-center justify-center gap-s2 rounded-r2 font-medium whitespace-nowrap no-underline disabled:cursor-not-allowed disabled:opacity-50";

// 尺寸不能靠外部类名覆盖，Tailwind 的先后顺序和类名的书写顺序无关。
const BUTTON_SIZES = {
    sm: "h-11 px-s3 text-t2 nav:h-8",
    md: "h-btn px-s4 text-t3",
    lg: "h-12 px-s5 text-t3",
    iconSm: "size-11 nav:size-8",
};

const BUTTON_VARIANTS = {
    primary: "bg-accent text-on-accent hover:bg-accent-hover disabled:hover:bg-accent",
    secondary: "bg-fill text-text hover:bg-fill-hover disabled:hover:bg-fill",
    ghost: "bg-transparent text-text-2 hover:bg-fill hover:text-text disabled:hover:bg-transparent",
    danger: "bg-danger-soft text-danger hover:bg-[color-mix(in_srgb,var(--danger)_14%,var(--danger-soft))]",
};

export type ButtonVariant = keyof typeof BUTTON_VARIANTS;
export type ButtonSize = keyof typeof BUTTON_SIZES;

export function buttonClass(variant: ButtonVariant = "secondary", size: ButtonSize = "md"): string {
    return `${BUTTON_BASE} ${BUTTON_SIZES[size]} ${BUTTON_VARIANTS[variant]}`;
}

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
    variant?: ButtonVariant;
    size?: ButtonSize;
}

export function Button({ variant, size, className = "", ...props }: ButtonProps) {
    return <button type="button" className={`${buttonClass(variant, size)} ${className}`} {...props} />;
}

export function LinkButton({
    variant,
    size,
    className = "",
    ...props
}: LinkProps & { variant?: ButtonVariant; size?: ButtonSize }) {
    return <Link className={`${buttonClass(variant, size)} ${className}`} {...props} />;
}

const CHIP_TONES = {
    ok: "bg-ok-soft text-ok",
    warn: "bg-warn-soft text-warn",
    danger: "bg-danger-soft text-danger",
    accent: "bg-accent-soft text-accent-text",
    neutral: "bg-fill text-text-2",
    testnet: "bg-testnet text-on-testnet",
};

export type ChipTone = keyof typeof CHIP_TONES;

export function Chip({ tone, className = "", children }: { tone: ChipTone; className?: string; children: ReactNode }) {
    return (
        <span
            className={`inline-flex h-6 items-center gap-s1 rounded-rf px-s2 text-t1 font-medium whitespace-nowrap ${CHIP_TONES[tone]} ${className}`}
        >
            {children}
        </span>
    );
}

/** 一组内容就是一张卡片。卡片不嵌套，嵌在里面时只保留内容。 */
export function Section({ className = "", children, ...props }: ComponentProps<"section">) {
    return (
        <section className={`card ${className}`} {...props}>
            {children}
        </section>
    );
}

/** 空状态。全站一个规格：居中一句话，最多一个操作。嵌在卡片里时只保留内容。 */
export function Empty({ icon, children, action }: { icon: IconName; children: ReactNode; action?: ReactNode }) {
    return (
        <Section className="min-h-40 items-center justify-center text-center">
            <span className="flex size-12 items-center justify-center rounded-full bg-surface-2 text-text-3">
                <Icon name={icon} size={24} />
            </span>
            <p className="text-t3 text-text-2">{children}</p>
            {action}
        </Section>
    );
}

/** 标题放进外壳的标题行，和网络切换、连接状态同一行。 */
export const TitleSlot = createContext<HTMLElement | null>(null);

export function PageTitle({ children }: { children: ReactNode }) {
    const slot = use(TitleSlot);
    if (!slot) return null;
    return createPortal(
        <h1 key={String(children)} className="fade-in truncate text-t5 font-semibold nav:text-t6">
            {children}
        </h1>,
        slot,
    );
}

export function SectionTitle({
    id,
    children,
    className = "",
}: {
    id?: string;
    children: ReactNode;
    className?: string;
}) {
    return (
        <h2 id={id} className={`text-t4 font-semibold ${className}`}>
            {children}
        </h2>
    );
}

export function Helper({ children, tone = "text-text-3" }: { children: ReactNode; tone?: string }) {
    return <p className={`text-t2 ${tone}`}>{children}</p>;
}

/** 错误和警告，读屏会立刻念出来。 */
export function Alert({
    children,
    tone = "text-danger",
    className = "",
}: {
    children: ReactNode;
    tone?: string;
    className?: string;
}) {
    return (
        <p role="alert" className={`text-t2 ${tone} ${className}`}>
            {children}
        </p>
    );
}

/** 表单标签。 */
export const LABEL = "text-t2 text-text-3";

/** 表格的列标题和行。文字贴着边线，悬停底色向两侧伸出。 */
export function ListHead({ grid, children }: { grid: string; children: ReactNode }) {
    return <div className={`${grid} row hidden h-8 items-center text-t2 text-text-3 @split:grid`}>{children}</div>;
}

export function Skeleton({ className = "" }: { className?: string }) {
    return <span aria-hidden="true" className={`skeleton ${className}`} />;
}

/** 读取中的列表。 */
export function SkeletonRows({ count = 3 }: { count?: number }) {
    return (
        <div aria-hidden="true" className="flex flex-col">
            {Array.from({ length: count }, (_, i) => i).map((i) => (
                <div key={i} className="flex h-row items-center gap-s3">
                    <Skeleton className="size-8 rounded-full" />
                    <span className="flex flex-1 flex-col gap-s2">
                        <Skeleton className="h-3 w-24" />
                        <Skeleton className="h-3 w-40" />
                    </span>
                    <Skeleton className="h-3 w-20" />
                </div>
            ))}
        </div>
    );
}

interface SegmentOption<T> {
    value: T;
    label: string;
    hint?: string | null;
}

interface SegmentedProps<T> {
    label: string;
    options: SegmentOption<T>[];
    value: T;
    onChange(value: T): void;
    size?: "sm" | "md" | "lg";
    stretch?: boolean;
}

const SEGMENT_SIZES = { sm: "h-11 px-s3 text-t2 nav:h-7", md: "h-11 px-s4 text-t2 nav:h-9", lg: "h-11 px-s4 text-t3" };

/** 分段选择，选中块滑到新位置，不是原地变色。 */
export function Segmented<T extends string>({
    label,
    options,
    value,
    onChange,
    size = "sm",
    stretch,
}: SegmentedProps<T>) {
    const index = options.findIndex((option) => option.value === value);
    const vars = { "--n": options.length, "--i": index } as CSSProperties;
    return (
        <fieldset aria-label={label} className={`segmented ${stretch ? "segmented-stretch" : ""}`} style={vars}>
            {index >= 0 ? <span aria-hidden="true" className="segmented-thumb" /> : null}
            {options.map((option) => (
                <button
                    key={option.value}
                    type="button"
                    aria-pressed={option.value === value}
                    onClick={() => onChange(option.value)}
                    className={`segmented-item font-medium whitespace-nowrap ${SEGMENT_SIZES[size]}`}
                >
                    {option.label}
                    {option.hint ? <span className="text-t1 font-normal text-text-3">{option.hint}</span> : null}
                </button>
            ))}
        </fieldset>
    );
}

/** 开关：圆点滑动，轨道变色。 */
export function Switch({
    checked,
    onChange,
    children,
}: {
    checked: boolean;
    onChange(value: boolean): void;
    children: ReactNode;
}) {
    return (
        <button
            type="button"
            role="switch"
            aria-checked={checked}
            onClick={() => onChange(!checked)}
            className="flex h-8 items-center gap-s2 text-t2 text-text-2"
        >
            <span aria-hidden="true" className="switch">
                <span className="switch-dot" />
            </span>
            {children}
        </button>
    );
}

/** 竖向单选列表，选中块滑到新位置。文字贴着边线，选中块向两侧伸出。 */
export function SelectList({
    selected,
    className = "",
    children,
}: {
    selected: number;
    className?: string;
    children: ReactNode;
}) {
    return (
        <div className={`select-rail -mx-s3 ${className}`} style={{ "--i": selected } as CSSProperties}>
            {selected >= 0 ? <span aria-hidden="true" className="select-thumb" /> : null}
            <ul className="relative">{children}</ul>
        </div>
    );
}

export function SelectRow({
    selected,
    onClick,
    children,
}: {
    selected: boolean;
    onClick(): void;
    children: ReactNode;
}) {
    return (
        <li>
            <button
                type="button"
                aria-pressed={selected}
                onClick={onClick}
                className="press flex h-row w-full items-center gap-s3 rounded-r2 px-s3 text-left text-text"
            >
                {children}
            </button>
        </li>
    );
}

/** 折叠区的开关，内容放在后面的 Collapse 里。 */
export function Disclosure({ open, onToggle, children }: { open: boolean; onToggle(): void; children: ReactNode }) {
    return (
        <button
            type="button"
            aria-expanded={open}
            onClick={onToggle}
            className="-mx-s3 flex h-btn items-center gap-s2 rounded-r2 px-s3 text-t2 text-text-2 transition-colors hover:bg-surface-2"
        >
            <Icon name="caret" size={16} className={`flip ${open ? "rotate-90" : ""}`} />
            {children}
        </button>
    );
}

/** 展开折叠，高度和透明度同时过渡。 */
export function Collapse({ open, children }: { open: boolean; children: ReactNode }) {
    return (
        <div className="fold" data-open={open ? "" : undefined}>
            <div>{children}</div>
        </div>
    );
}

/** 标签和值一行，靠行距分组，不画分隔线。 */
export function Fact({ label, children, mono }: { label: string; children: ReactNode; mono?: boolean }) {
    return (
        <div className="flex min-h-8 items-baseline justify-between gap-s4 py-s1">
            <dt className="shrink-0 text-text-3">{label}</dt>
            <dd className={`min-w-0 text-right [overflow-wrap:anywhere] ${mono ? "font-mono" : ""}`}>{children}</dd>
        </div>
    );
}

/** 完整地址，4 个字符一组，奇数组用 text，偶数组用 text-2。 */
export function FullAddress({ address, className = "" }: { address: string; className?: string }) {
    return (
        <span className={`flex flex-wrap gap-x-s2 gap-y-s1 font-mono ${className}`}>
            {groupAddress(address).map((group, i) => (
                <span key={group.offset} className={i % 2 === 0 ? "text-text" : "text-text-2"}>
                    {group.text}
                </span>
            ))}
        </span>
    );
}

/** 「至 0x7a3F…9C21」这样的文字，只有地址用等宽字体。 */
export function Party({ text }: { text: string }) {
    const space = text.indexOf(" ");
    if (space < 0) return <span>{text}</span>;
    return (
        <span>
            {text.slice(0, space + 1)}
            <span className="font-mono">{text.slice(space + 1)}</span>
        </span>
    );
}
