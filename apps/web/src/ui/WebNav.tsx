import { Menu } from "@base-ui/react/menu";
import type { CSSProperties } from "react";
import { Link, useLocation, useNavigate } from "react-router";
import { useApp } from "../data/app";
import { BrandMark } from "./BrandMark";
import { Icon, type IconName } from "./Icon";
import { modeLabel } from "./labels";
import { Chip } from "./primitives";
import { WebTop } from "./WebTop";
import { t } from "../core/i18n";

/** match 里的页面都算这一项，转账流程走到签名和广播时选中态留在「转账」。 */
const ITEMS: { to: string; label: string; icon: IconName; match: string[] }[] = [
    { to: "/", label: t("资产"), icon: "assets", match: ["/", "/receive"] },
    { to: "/send", label: t("转账"), icon: "send", match: ["/send", "/sign", "/broadcast"] },
    { to: "/history", label: t("交易记录"), icon: "history", match: ["/history"] },
    { to: "/contacts", label: t("地址簿"), icon: "contacts", match: ["/contacts"] },
    { to: "/device", label: t("冷钱包"), icon: "device", match: ["/device", "/pair"] },
    { to: "/settings", label: t("设置"), icon: "settings", match: ["/settings"] },
];

function WalletMenu() {
    const { wallets, wallet, mode, selectWallet } = useApp();
    const navigate = useNavigate();
    const inMode = wallets.filter((w) => w.mode === mode);
    return (
        <Menu.Root>
            <Menu.Trigger className="press flex h-11 max-w-44 min-w-0 items-center gap-s2 rounded-r2 bg-fill px-s3 text-left hover:bg-fill-hover data-[popup-open]:bg-fill-hover">
                <span className="flex min-w-0 flex-1 items-baseline gap-s2">
                    <span className="truncate text-t2 font-medium">{wallet?.name ?? t("未配对")}</span>
                    <span className="hidden shrink-0 font-mono text-t1 text-text-3 nav:inline">
                        {wallet?.fingerprint ?? "--------"}
                    </span>
                </span>
                <Icon name="updown" size={16} className="shrink-0 text-text-3" />
            </Menu.Trigger>
            <Menu.Portal>
                <Menu.Positioner sideOffset={8} align="end" className="z-20">
                    <Menu.Popup className="popup min-w-[max(var(--anchor-width),15rem)]">
                        {inMode.map((item) => (
                            <Menu.Item
                                key={item.id}
                                onClick={() => void selectWallet(item)}
                                className="popup-item cursor-pointer"
                            >
                                {/* 三列都固定：名称占满并截断，指纹宽度相同，勾选位始终留着。 */}
                                <span className="min-w-0 flex-1 truncate font-medium">{item.name}</span>
                                <span className="shrink-0 font-mono text-t1 text-text-3">{item.fingerprint}</span>
                                <span className="flex size-4 shrink-0 items-center justify-center text-accent-text">
                                    {item.id === wallet?.id ? <Icon name="check" size={16} /> : null}
                                </span>
                            </Menu.Item>
                        ))}
                        <Menu.Item
                            onClick={() => void navigate("/pair")}
                            className="popup-item cursor-pointer font-medium text-accent-text"
                        >
                            {t("配对新钱包")}
                        </Menu.Item>
                    </Menu.Popup>
                </Menu.Positioner>
            </Menu.Portal>
        </Menu.Root>
    );
}

/** 标志右侧的网络标签，点一下切到另一个网络。只换查看的余额，一次配对两个网络都有。 */
function NetworkTag() {
    const { wallets, mode, setMode } = useApp();
    if (wallets.length === 0) return null;
    const other = mode === "testnet" ? "mainnet" : "testnet";
    return (
        <button
            type="button"
            aria-label={t("{0}，切换到{1}", modeLabel(mode), modeLabel(other))}
            title={t("切换到{0}", modeLabel(other))}
            onClick={() => void setMode(other)}
            className="press flex min-h-11 shrink-0 items-center"
        >
            {/* 宽度固定，切换网络时导航不移动。 */}
            <Chip tone={mode === "testnet" ? "testnet" : "neutral"} className="w-22 justify-center">
                {modeLabel(mode)}
                <Icon name="updown" size={12} />
            </Chip>
        </button>
    );
}

/** 桌面端的顶部导航。窄屏时导航在底部标签栏，这里只留标志和钱包。 */
export function WebNav() {
    const { pathname } = useLocation();
    const active = ITEMS.findIndex((item) => item.match.includes(pathname));
    return (
        <header className="topbar">
            <div className="page-frame flex h-full items-center gap-s6">
                <div className="flex shrink-0 items-center gap-s2">
                    <Link
                        to="/"
                        aria-label="BitOne"
                        className="flex min-h-11 shrink-0 items-center gap-s2 text-text no-underline"
                    >
                        <BrandMark size={28} />
                        <span className="hidden text-t4 font-semibold nav:inline">BitOne</span>
                    </Link>
                    <NetworkTag />
                </div>
                <nav
                    aria-label={t("主导航")}
                    className="hidden min-w-0 flex-1 items-center gap-s6 self-stretch nav:flex"
                >
                    {ITEMS.map((item, i) => (
                        <Link
                            key={item.to}
                            to={item.to}
                            aria-current={item.to === pathname ? "page" : undefined}
                            data-on={i === active ? "" : undefined}
                            className="top-link"
                        >
                            {item.label}
                        </Link>
                    ))}
                </nav>
                <div className="ml-auto flex min-w-0 items-center gap-s2 nav:ml-0 nav:gap-s3">
                    <WebTop />
                    <WalletMenu />
                </div>
            </div>
        </header>
    );
}

/** 手机底部标签栏。放不下的两项收进「更多」。 */
const TABS = ITEMS.slice(0, 4);
const MORE = ITEMS.slice(4);

export function TabBar() {
    const { pathname } = useLocation();
    const navigate = useNavigate();
    const tab = TABS.findIndex((item) => item.match.includes(pathname));
    const inMore = MORE.some((item) => item.match.includes(pathname));
    const index = inMore ? TABS.length : tab;
    const vars = { "--n": TABS.length + 1, "--i": index } as CSSProperties;
    return (
        <nav aria-label={t("底部导航")} className="tabbar nav:hidden" style={vars}>
            {index >= 0 ? <span aria-hidden="true" className="tabbar-mark" /> : null}
            {TABS.map((item, i) => (
                <Link
                    key={item.to}
                    to={item.to}
                    aria-current={item.to === pathname ? "page" : undefined}
                    data-on={i === tab ? "" : undefined}
                    className="tabbar-item press"
                >
                    <Icon name={item.icon} />
                    {item.label}
                </Link>
            ))}
            <Menu.Root>
                <Menu.Trigger data-on={inMore ? "" : undefined} className="tabbar-item press">
                    <Icon name="more" />
                    {t("更多")}
                </Menu.Trigger>
                <Menu.Portal>
                    <Menu.Positioner side="top" sideOffset={8} align="end" className="z-20">
                        <Menu.Popup className="popup min-w-40">
                            {MORE.map((item) => (
                                <Menu.Item
                                    key={item.to}
                                    onClick={() => void navigate(item.to)}
                                    className="popup-item cursor-pointer gap-s3"
                                >
                                    <Icon name={item.icon} />
                                    {item.label}
                                </Menu.Item>
                            ))}
                        </Menu.Popup>
                    </Menu.Positioner>
                </Menu.Portal>
            </Menu.Root>
        </nav>
    );
}
