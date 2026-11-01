import { chainOf } from "../chains";
import { useMemo, useState } from "react";
import { useLocation, useSearchParams } from "react-router";
import { addressFrame, networkName, type Mode } from "../core";
import { shortAddress } from "../core/text";
import { saveSetting, useApp, useLive } from "../data/app";
import { db, type Address, type Wallet } from "../data/db";
import { addressPath, ensureAddress } from "../data/pairing";
import { receiveRows, type ReceiveRow } from "../gateway/assets";
import { accountKind } from "../ui/labels";
import { Picker } from "../ui/Picker";
import { Button, Chip, Fact, FullAddress, LABEL, Notice, SelectList, SelectRow, SkeletonRows } from "../ui/primitives";
import { QrCode } from "../ui/QrCode";
import { Sheet, SheetActions } from "../ui/Sheet";
import { TokenIcon, networkIcon } from "../ui/TokenIcon";
import { t } from "../core/i18n";

/** 发放过的地址,最新的在前. */
function useIssued(accountId: string) {
    const { addresses } = useApp();
    const indexKey = `receive:${accountId}`;
    const stored = useLive(() => db.settings.get(indexKey), [indexKey]);
    const latest = typeof stored?.value === "number" ? stored.value : 0;
    const issued = addresses
        .filter((item) => item.accountId === accountId && item.change === 0 && item.index <= latest)
        .sort((a, b) => b.index - a.index);
    return { indexKey, latest, issued };
}

function AssetLine({ row }: { row: ReceiveRow }) {
    return (
        <>
            <TokenIcon asset={row.entry} size={24} />
            <span className="truncate text-t3 font-medium">{row.entry.symbol}</span>
        </>
    );
}

function NetworkLine({ row }: { row: ReceiveRow }) {
    return (
        <>
            <TokenIcon network={networkIcon(row.network)} size={24} />
            <span className="flex min-w-0 flex-col">
                <span className="text-t3 leading-5 font-medium">{networkName(row.network)}</span>
                {row.account.kind === "single" ? null : (
                    <span className="truncate text-t1 text-text-3">{accountKind(row.account)}</span>
                )}
            </span>
        </>
    );
}

type View = "main" | "verify" | "list";

interface PanelProps {
    wallet: Wallet;
    row: ReceiveRow;
    mode: Mode;
    view: View;
    setView(view: View): void;
    onIndex(index: number | null): void;
    index: string | null;
}

/** 一个账户的收款地址.收款,核对,地址列表在这里切换,不离开浮层. */
function AddressPanel({ wallet, row: asset, mode, view, setView, index, onIndex }: PanelProps) {
    const { account } = asset;
    const { indexKey, latest, issued } = useIssued(account.id);
    const row = issued.find((item) => String(item.index) === index) ?? issued[0];
    const [copied, setCopied] = useState(false);
    const [error, setError] = useState("");

    const path = row ? addressPath(account, row) : "";
    const frame = useMemo(
        () => (row ? addressFrame(wallet.fingerprint, account.network, path, row.address) : ""),
        [row, wallet.fingerprint, account.network, path],
    );

    async function copy(address: string) {
        try {
            await navigator.clipboard.writeText(address);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        } catch {
            setError(t("浏览器不允许复制,请手动选择地址."));
        }
    }

    async function next() {
        setError("");
        try {
            await ensureAddress(account, mode, latest + 1);
            await saveSetting(indexKey, latest + 1);
            onIndex(null);
            setView("main");
        } catch {
            setError(t("无法生成新地址."));
        }
    }

    async function confirm(address: Address) {
        await db.addresses.update([account.id, 0, address.index], { verified: true });
        setView("main");
    }

    if (!row) return <SkeletonRows count={2} />;

    if (view === "list") {
        return (
            <div className="flex min-w-0 flex-col gap-s4">
                <SelectList selected={issued.findIndex((item) => item.index === row.index)}>
                    {issued.map((item) => (
                        <SelectRow
                            key={item.index}
                            selected={item.index === row.index}
                            onClick={() => {
                                onIndex(item.index);
                                setView("main");
                            }}
                        >
                            <span className="min-w-0 flex-1 truncate font-mono text-t2">
                                {shortAddress(item.address)}
                            </span>
                            {item.index === latest ? <Chip tone="accent">{t("最新")}</Chip> : null}
                        </SelectRow>
                    ))}
                </SelectList>
                <Notice>{t("使用新地址后,旧地址仍然可以收款.")}</Notice>
                {error ? <Notice tone="danger">{error}</Notice> : null}
                <SheetActions fill>
                    <Button onClick={() => setView("main")}>{t("返回")}</Button>
                    <Button variant="primary" onClick={() => void next()}>
                        {t("使用新地址")}
                    </Button>
                </SheetActions>
            </div>
        );
    }

    const verifying = view === "verify";
    return (
        <div className="flex min-w-0 flex-col gap-s4">
            {/* 手机上按屏幕高度收窄,核对和收款都不用滚动.30rem 是浮层里除了二维码的高度. */}
            <div className="mx-auto flex w-full max-w-[max(11rem,calc(100dvh-30rem-var(--safe-top)-var(--safe-bottom)))] justify-center">
                {verifying ? (
                    <QrCode value={frame} size={297} label={t("地址核对二维码")} />
                ) : (
                    <QrCode value={row.address} plain size={297} label={t("收款地址二维码")} />
                )}
            </div>
            <div className="flex flex-col gap-s1">
                {/* 高度按地址列表的入口留,换账户时下面的内容不动. */}
                <div className="flex min-h-11 items-center justify-between gap-s3 nav:min-h-8">
                    <span className="flex min-w-0 items-center gap-s3">
                        <span className={LABEL}>{t("地址")}</span>
                        {chainOf(account.chain).hdAddresses && !verifying ? (
                            <button type="button" className="link" onClick={() => setView("list")}>
                                {t("地址列表")}
                            </button>
                        ) : null}
                    </span>
                    <span key={String(row.verified)} className="fade-in">
                        <Chip tone={row.verified ? "ok" : "warn"}>{row.verified ? t("已核对") : t("未核对")}</Chip>
                    </span>
                </div>
                {/* 最长的地址也是两行,短地址留出同样的高度,换账户时下面的内容不跳. */}
                <FullAddress address={row.address} className="min-h-[calc(2lh+var(--s1))] content-start text-t2" />
            </div>
            {verifying ? (
                <dl className="flex flex-col text-t2">
                    <Fact label={t("网络")}>
                        {networkName(asset.network)} {accountKind(account)}
                    </Fact>
                    <Fact label={t("路径")} mono>
                        {path}
                    </Fact>
                </dl>
            ) : null}
            {verifying ? (
                <Notice tone="warn">{t("用冷钱包扫这个码,不要发给付款方.显示属于本钱包并且逐段一致,才算通过.")}</Notice>
            ) : row.verified ? null : (
                <Notice tone="warn">{t("收款前先在冷钱包核对地址,以冷钱包屏幕为准.")}</Notice>
            )}
            {error ? <Notice tone="danger">{error}</Notice> : null}
            <SheetActions fill>
                {verifying ? (
                    <>
                        <Button onClick={() => setView("main")}>{t("返回")}</Button>
                        <Button variant="primary" onClick={() => void confirm(row)}>
                            {t("地址一致")}
                        </Button>
                    </>
                ) : row.verified ? (
                    <>
                        <Button onClick={() => setView("verify")}>{t("重新核对")}</Button>
                        <Button variant="primary" onClick={() => void copy(row.address)}>
                            {copied ? t("已复制") : t("复制地址")}
                        </Button>
                    </>
                ) : (
                    <>
                        <Button onClick={() => void copy(row.address)}>{copied ? t("已复制") : t("复制地址")}</Button>
                        <Button variant="primary" onClick={() => setView("verify")}>
                            {t("冷钱包核对")}
                        </Button>
                    </>
                )}
            </SheetActions>
        </div>
    );
}

/** 收款是动作,不是页面.从哪里打开,关掉就回到哪里.选的是资产和网络,地址跟着账户走. */
export default function ReceiveDialog() {
    const { wallet, accounts, mode } = useApp();
    const [params, setParams] = useSearchParams();
    const { state } = useLocation();
    const rows = receiveRows(accounts, mode);
    const [view, setView] = useState<View>(params.has("verify") ? "verify" : "main");
    // 网址只是意向,资产优先,其次账户,最后网络.都对不上时落到第一行.
    const fit = (item: ReceiveRow) =>
        (item.entry.icon === params.get("asset") ? 4 : 0) +
        (item.account.id === params.get("receive") ? 2 : 0) +
        (item.network === params.get("network") ? 1 : 0);
    const row = rows.reduce<ReceiveRow | undefined>(
        (best, item) => (best && fit(best) >= fit(item) ? best : item),
        undefined,
    );

    function update(change: (next: URLSearchParams) => void) {
        const next = new URLSearchParams(params);
        change(next);
        setParams(next, { replace: true, state });
    }

    function pick(change: Record<string, string>) {
        update((next) => {
            for (const [key, value] of Object.entries(change)) next.set(key, value);
            next.delete("index");
            next.delete("verify");
        });
        setView("main");
    }

    return (
        <Sheet title={row ? t("收款 {0}", row.entry.symbol) : t("收款")}>
            {wallet && row ? (
                <div className="flex min-w-0 flex-col gap-s4">
                    {/* 面板按账户重建,选择器放在外面,换账户时才放得完离场动效.核对和地址列表只看地址,不显示. */}
                    {view === "main" ? (
                        <div className="grid grid-cols-[2fr_3fr] gap-s2">
                            <Picker
                                id="receive-asset"
                                label={t("资产")}
                                value={<AssetLine row={row} />}
                                items={[...new Map(rows.map((item) => [item.entry.icon, item])).values()].map(
                                    (item) => ({
                                        key: item.entry.icon,
                                        node: <AssetLine row={item} />,
                                        selected: item.entry.icon === row.entry.icon,
                                    }),
                                )}
                                onPick={(icon) => pick({ asset: icon })}
                            />
                            <Picker
                                id="receive-network"
                                label={t("网络")}
                                value={<NetworkLine row={row} />}
                                items={rows
                                    .filter((item) => item.entry.icon === row.entry.icon)
                                    .map((item) => ({
                                        key: item.id,
                                        node: <NetworkLine row={item} />,
                                        selected: item.id === row.id,
                                    }))}
                                onPick={(id) => {
                                    const item = rows.find((other) => other.id === id);
                                    if (item) pick({ receive: item.account.id, network: item.network });
                                }}
                            />
                        </div>
                    ) : null}
                    <AddressPanel
                        key={row.account.id}
                        wallet={wallet}
                        row={row}
                        mode={mode}
                        view={view}
                        setView={setView}
                        index={params.get("index")}
                        onIndex={(index) =>
                            update((next) => {
                                if (index === null) next.delete("index");
                                else next.set("index", String(index));
                            })
                        }
                    />
                </div>
            ) : (
                <SkeletonRows />
            )}
        </Sheet>
    );
}
