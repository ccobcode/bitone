// 界面功能图标,线宽 1.8,圆角端点,24 网格.路径和冷钱包共用,来源是 design/icons.json.
import PATHS from "./icons.gen.json";

export type IconName = keyof typeof PATHS;

interface IconProps {
    name: IconName;
    size?: number;
    className?: string;
    /** 完成时描线. */
    draw?: boolean;
}

/** 下载中:箭头往托盘里落,循环.加载不用转圈,见 docs/06-ui.md 第 5 节. */
export function DownloadingIcon() {
    return (
        <svg
            width={16}
            height={16}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
        >
            <path d="M12 4v11M7 10l5 5 5-5" className="icon-drop" />
            <path d="M5 20h14" />
        </svg>
    );
}

export function Icon({ name, size = 18, className, draw }: IconProps) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
            className={className}
        >
            <path d={PATHS[name]} pathLength={draw ? 1 : undefined} className={draw ? "draw" : undefined} />
        </svg>
    );
}
