import { Link, Navigate } from "react-router";
import { useApp } from "../data/app";
import { BrandMark } from "../ui/BrandMark";
import { Icon } from "../ui/Icon";
import { t } from "../core/i18n";

const ENTRY = "press group flex min-h-20 items-center gap-s4 rounded-r3 px-s5 text-t4 font-semibold no-underline";

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
    if (wallets.length > 0) return <Navigate to="/" replace />;
    return (
        <div className="flex min-h-dvh flex-col">
            <main className="flex flex-1 flex-col items-center justify-center px-(--page-x) py-s12">
                <div className="stagger flex w-full max-w-100 flex-col gap-s6">
                    <div className="flex items-center gap-s2">
                        <BrandMark size={28} />
                        <span className="text-t4 font-semibold">BitOne</span>
                    </div>

                    <h1 className="text-t7 font-semibold">{t("连接你的冷钱包")}</h1>

                    <div className="flex flex-col gap-s3">
                        <Link to="/pair" className={`${ENTRY} bg-accent text-on-accent hover:bg-accent-hover`}>
                            <Icon name="qr" size={24} />
                            <span className="flex-1">{t("扫码连接")}</span>
                            <EntryArrow />
                        </Link>
                        <Link
                            to="/settings"
                            className={`${ENTRY} border border-line bg-surface text-text hover:border-line-strong`}
                        >
                            <Icon name="file" size={24} className="text-text-2" />
                            <span className="flex-1">{t("导入备份文件")}</span>
                            <EntryArrow />
                        </Link>
                    </div>
                </div>
            </main>
        </div>
    );
}
