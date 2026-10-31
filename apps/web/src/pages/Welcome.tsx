import { Link, Navigate, useSearchParams } from "react-router";
import { useApp } from "../data/app";
import { BrandMark } from "../ui/BrandMark";
import { Icon } from "../ui/Icon";
import { LanguageSwitch } from "../ui/LanguageSwitch";
import { Overlays } from "../ui/Overlays";
import { overlayLink } from "../ui/Sheet";
import { t } from "../core/i18n";

const ENTRY =
    "press group flex min-h-12 items-center gap-s3 rounded-r3 px-s4 text-t3 font-semibold no-underline nav:min-h-20 nav:gap-s4 nav:px-s5 nav:text-t4";

function EntryArrow() {
    return (
        <Icon
            name="arrow"
            size={20}
            className="shrink-0 transition-transform duration-(--dur-normal) group-hover:translate-x-s1"
        />
    );
}

export default function Welcome() {
    const { wallets } = useApp();
    const [params] = useSearchParams();
    // 配对和导入写入钱包后，浮层还要走完自己的最后一步，由它负责离开这一页。
    const busy = params.has("connect") || params.has("import");
    if (wallets.length > 0 && !busy) return <Navigate to="/" replace />;
    return (
        <div className="relative flex min-h-dvh flex-col">
            <div className="absolute top-[calc(var(--safe-top)+var(--s4))] right-(--page-x)">
                <LanguageSwitch />
            </div>
            <main className="flex flex-1 flex-col items-center justify-center px-(--page-x) py-s12">
                <div className="stagger flex w-full max-w-100 flex-col gap-s6">
                    <div className="flex items-center gap-s2">
                        <BrandMark size={28} />
                        <span className="text-t4 font-semibold">BitOne</span>
                    </div>

                    <h1 className="text-t7 font-semibold">{t("连接你的冷钱包")}</h1>

                    <div className="flex flex-col gap-s3">
                        <Link
                            {...overlayLink("?connect=1")}
                            className={`${ENTRY} bg-accent text-on-accent hover:bg-accent-hover`}
                        >
                            <Icon name="qr" size={24} />
                            <span className="flex-1">{t("扫码连接")}</span>
                            <EntryArrow />
                        </Link>
                        <Link
                            {...overlayLink("?import=1")}
                            className={`${ENTRY} border border-line bg-surface text-text hover:border-line-strong`}
                        >
                            <Icon name="file" size={24} className="text-text-2" />
                            <span className="flex-1">{t("导入备份文件")}</span>
                            <EntryArrow />
                        </Link>
                    </div>
                </div>
            </main>
            <Overlays />
        </div>
    );
}
