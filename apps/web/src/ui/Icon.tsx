// 界面功能图标，线宽 1.8，圆角端点，24 网格。路径来自原型。
const PATHS = {
    assets: "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
    send: "M7 17L17 7M8 7h9v9",
    receive: "M17 7L7 17M16 17H7V8",
    history: "M12 7v5l3 2M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z",
    contacts: "M5 4h11a3 3 0 0 1 3 3v13H8a3 3 0 0 1-3-3zM5 17a3 3 0 0 1 3-3h11",
    device: "M8 3h8a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H8a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM11 18h2",
    settings:
        "M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1M13 6a2 2 0 1 0 4 0a2 2 0 1 0-4 0M7 12a2 2 0 1 0 4 0a2 2 0 1 0-4 0M15 18a2 2 0 1 0 4 0a2 2 0 1 0-4 0",
    wallet: "M3 7a2 2 0 0 1 2-2h13v4M3 7v11a2 2 0 0 0 2 2h15V9H5a2 2 0 0 1-2-2zM16 14h.01",
    updown: "M8 9l4-4 4 4M8 15l4 4 4-4",
    search: "M18 11a7 7 0 1 1-14 0 7 7 0 0 1 14 0zM20 20l-3.5-3.5",
    qr: "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h3v3h-3zM20 14v.01M14 20h.01M17 20h4v-3",
    file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5M12 11v6M9 14l3 3 3-3",
    arrow: "M5 12h14M13 6l6 6-6 6",
    noAccount: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM17 8l5 5M22 8l-5 5",
    server: "M4 4h16v6H4zM4 14h16v6H4zM8 7h.01M8 17h.01",
    lock: "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
    database:
        "M4 5c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
    offline: "M2 2l20 20M8.5 16.5a5 5 0 0 1 7 0M5 13a10 10 0 0 1 5.2-2.8M19 13a10 10 0 0 0-2.4-1.8M12 20h.01",
    cameraOff: "M2 2l20 20M9 5h6l2 3h3v10M4 8v11h13M12 17a4 4 0 0 1-3.5-5.9",
    download: "M12 4v11M7 10l5 5 5-5M5 20h14",
    upload: "M12 15V4M7 9l5-5 5 5M5 20h14",
    expand: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5",
    copy: "M9 9h11v11H9zM5 15H4V4h11v1",
    external: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    check: "M5 12l5 5L20 7",
    alert: "M12 3l9 16H3zM12 10v4M12 17h.01",
    unknown: "M12 8a3 3 0 0 1 2.1 5.1c-.8.8-2.1 1.3-2.1 2.9M12 19h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z",
    close: "M6 6l12 12M18 6L6 18",
    clock: "M12 7v5l3 2M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z",
    camera: "M4 8h3l2-3h6l2 3h3v11H4zM12 17a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
    scan: "M3 7V5a2 2 0 0 1 2-2h2M17 3h2a2 2 0 0 1 2 2v2M21 17v2a2 2 0 0 1-2 2h-2M7 21H5a2 2 0 0 1-2-2v-2M7 12h10",
    chevron: "M6 9l6 6 6-6",
    caret: "M9 6l6 6-6 6",
} as const;

export type IconName = keyof typeof PATHS;

interface IconProps {
    name: IconName;
    size?: number;
    className?: string;
    /** 完成时描线。 */
    draw?: boolean;
}

/** 下载中：箭头往托盘里落，循环。加载不用转圈，见 docs/06-ui.md 第 5 节。 */
export function DownloadingIcon({ size = 16 }: { size?: number }) {
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
