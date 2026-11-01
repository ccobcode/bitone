// 公共小组件.尺寸和颜色都来自令牌.
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
import { t } from "../core/i18n";
import { groupAddress } from "../core/text";
import { Icon, type IconName } from "./Icon";

const BUTTON_BASE =
    "press inline-flex shrink-0 items-center justify-center gap-s2 rounded-r2 font-medium whitespace-nowrap no-underline disabled:cursor-not-allowed disabled:text-text-3";

// 尺寸不能靠外部类名覆盖,Tailwind 的先后顺序和类名的书写顺序无关.
const BUTTON_SIZES = {
    sm: "h-11 px-s3 text-t2 nav:h-8 nav:rounded-r1",
    md: "h-btn px-s4 text-t3",
    lg: "h-btn px-s5 text-t3 nav:h-12",
    iconSm: "size-11 nav:size-8 nav:rounded-r1",
};

const BUTTON_VARIANTS = {
    primary: "bg-accent text-on-accent hover:bg-accent-hover disabled:bg-surface-3 disabled:hover:bg-surface-3",
    secondary: "bg-fill text-text hover:bg-fill-hover disabled:hover:bg-fill",
    ghost: "bg-transparent text-text-2 hover:bg-fill hover:text-text disabled:hover:bg-transparent",
    danger: "bg-danger-soft text-danger hover:bg-[color-mix(in_srgb,var(--danger)_14%,var(--danger-soft))] disabled:bg-surface-3 disabled:hover:bg-surface-3",
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

/** 一组内容就是一张卡片.卡片不嵌套,嵌在里面时只保留内容. */
export function Section({ className = "", children, ...props }: ComponentProps<"section">) {
    return (
        <section className={`card ${className}`} {...props}>
            {children}
        </section>
    );
}

/** 空状态.全站一个规格:居中一句话,最多一个操作.嵌在卡片里时只保留内容. */
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

/** 标题放进外壳的标题行,和网络切换,连接状态同一行. */
export const TitleSlot = createContext<HTMLElement | null>(null);

export function PageTitle({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
    const slot = use(TitleSlot);
    if (!slot) return null;
    return createPortal(
        <>
            <h1 key={String(children)} className="fade-in min-w-0 truncate text-t6 font-semibold">
                {children}
            </h1>
            {aside}
        </>,
        slot,
    );
}

/** 子页面的标题:父级链接加当前对象,导航的选中态留在父级.手机上是返回按钮加标题,和冷钱包的标题栏相同. */
export function ChildTitle({ parent, children }: { parent: { to: string; label: string }; children: ReactNode }) {
    const slot = use(TitleSlot);
    if (!slot) return null;
    return createPortal(
        <nav
            aria-label={t("页面层级")}
            className="fade-in flex min-h-11 min-w-0 items-center gap-s2 text-t4 nav:min-h-8 nav:text-t2"
        >
            <Link
                to={parent.to}
                aria-label={parent.label}
                className="press -ml-3 flex size-11 shrink-0 items-center justify-center gap-s1 text-text no-underline hover:text-text nav:ml-0 nav:size-auto nav:text-text-3"
            >
                <Icon name="back" size={24} className="nav:hidden" />
                <Icon name="back" size={14} className="hidden nav:block" />
                <span className="hidden nav:inline">{parent.label}</span>
            </Link>
            <span aria-hidden="true" className="hidden text-line-strong nav:inline">
                /
            </span>
            <h1 className="truncate font-semibold nav:font-medium nav:text-text-2">{children}</h1>
        </nav>,
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

/** 字段或区块标题的一句说明,紧贴它说明的东西.独立成段要提醒用户的话用 Notice. */
export function Helper({ children, tone = "text-text-3" }: { children: ReactNode; tone?: string }) {
    return <p className={`text-t2 ${tone}`}>{children}</p>;
}

const NOTICE_TONES = {
    info: { block: "bg-surface-2 text-text-2", icon: "unknown" },
    warn: { block: "bg-warn-soft text-warn", icon: "alert" },
    danger: { block: "bg-danger-soft text-danger", icon: "alert" },
} as const;

/** 告示:用户要停下来看的事.贴着它说明的内容,放在区块标题下面或操作按钮上面.警告和错误读屏会立刻念出来. */
export function Notice({
    tone = "info",
    className = "",
    children,
}: {
    tone?: keyof typeof NOTICE_TONES;
    className?: string;
    children: ReactNode;
}) {
    const { block, icon } = NOTICE_TONES[tone];
    return (
        <div
            role={tone === "info" ? "status" : "alert"}
            className={`rise flex items-start gap-s2 rounded-r2 px-s3 py-s2 text-t2 ${block} ${className}`}
        >
            <Icon name={icon} size={16} className="mt-0.5 shrink-0" />
            <div className="min-w-0 [overflow-wrap:anywhere]">{children}</div>
        </div>
    );
}

export const LABEL = "text-t2 text-text-3";

interface ListRowProps {
    icon: ReactNode;
    title: ReactNode;
    sub?: ReactNode;
    /** 标题和值之间的标签,窄屏不显示. */
    badge?: ReactNode;
    /** 行内操作.宽屏悬停才出现.窄屏放不下时不显示,整行可点或者右边有值都算放不下. */
    actions?: ReactNode;
    /** 右侧的值,右对齐. */
    value?: ReactNode;
    /** 整行的链接,带箭头. */
    link?: LinkProps;
    label?: string;
}

/** 列表的一行,全站一个规格.文字贴着卡片内边距,悬停底色向两侧伸出. */
export function ListRow({ icon, title, sub, badge, actions, value, link, label }: ListRowProps) {
    return (
        <li className="row relative flex min-h-row items-center gap-s3 py-s1">
            {link ? <Link {...link} aria-label={label} className="absolute inset-0 rounded-r2" /> : null}
            {icon}
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-t3 font-medium">{title}</span>
                {sub ? <span className="block min-w-0 truncate text-t2 text-text-3">{sub}</span> : null}
            </span>
            {badge ? <span className="hidden shrink-0 @split:block">{badge}</span> : null}
            {actions ? (
                <span
                    className={`row-actions relative shrink-0 gap-s1 ${link || value ? "hidden @split:flex" : "flex"}`}
                >
                    {actions}
                </span>
            ) : null}
            {value ? <span className="flex min-w-0 shrink-0 flex-col items-end text-right">{value}</span> : null}
            {link ? <Icon name="caret" size={16} className="shrink-0 text-text-3" /> : null}
        </li>
    );
}

export function Skeleton({ className = "" }: { className?: string }) {
    return <span aria-hidden="true" className={`skeleton ${className}`} />;
}

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

/** 分段选择,选中块滑到新位置,不是原地变色. */
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

/** 开关:圆点滑动,轨道变色.放在设置行里时整行都能点,和冷钱包相同. */
export function Switch({
    checked,
    onChange,
    label,
}: {
    checked: boolean;
    onChange(value: boolean): void;
    label: string;
}) {
    return (
        <button
            type="button"
            role="switch"
            aria-checked={checked}
            aria-label={label}
            onClick={() => onChange(!checked)}
            className="switch-button flex min-h-8 items-center"
        >
            <span aria-hidden="true" className="switch">
                <span className="switch-dot" />
            </span>
        </button>
    );
}

/** 竖向单选列表,选中块滑到新位置.文字贴着边线,选中块向两侧伸出. */
export function SelectList({ selected, children }: { selected: number; children: ReactNode }) {
    return (
        <div className="select-rail -mx-s3" style={{ "--i": selected } as CSSProperties}>
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

/** 折叠区的开关,内容放在后面的 Collapse 里.整行可点,箭头在右边翻转,和冷钱包相同. */
export function Disclosure({ open, onToggle, children }: { open: boolean; onToggle(): void; children: ReactNode }) {
    return (
        <button
            type="button"
            aria-expanded={open}
            onClick={onToggle}
            className="press -mx-s3 flex h-11 items-center justify-between gap-s2 rounded-r2 px-s3 text-t2 text-text-2 hover:bg-surface-2 nav:h-btn"
        >
            {children}
            <Icon name="chevron" size={16} className="flip shrink-0 text-text-3" />
        </button>
    );
}

/** 展开折叠,高度和透明度同时过渡. */
export function Collapse({ open, children }: { open: boolean; children: ReactNode }) {
    return (
        <div className="fold" data-open={open ? "" : undefined}>
            <div>{children}</div>
        </div>
    );
}

/** 手机上 t7 一行放得下的字数,图标占一列时也放得下. */
const BIG_AMOUNT = 13;

/** 这一屏的主金额.手机上放不下一行时缩一档,金额不截断. */
export function BigAmount({ children }: { children: string }) {
    const size = children.length > BIG_AMOUNT ? "text-t5 @split:text-t7" : "text-t7";
    return <span className={`font-semibold [overflow-wrap:anywhere] ${size}`}>{children}</span>;
}

/** 标签和值一行,靠行距分组,不画分隔线. */
export function Fact({ label, children, mono }: { label: string; children: ReactNode; mono?: boolean }) {
    return (
        <div className="flex min-h-8 items-baseline justify-between gap-s4 py-s1">
            <dt className="shrink-0 text-text-3">{label}</dt>
            <dd className={`min-w-0 text-right [overflow-wrap:anywhere] ${mono ? "font-mono" : ""}`}>{children}</dd>
        </div>
    );
}

/** 完整地址,4 个字符一组,奇数组用 text,偶数组用 text-2. */
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

/** "至 0x7a3F…9C21"这样的文字,只有地址用等宽字体. */
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
