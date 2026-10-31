import { useMemo, useState } from "react";
import { useLocation, useSearchParams } from "react-router";
import { addressFrame, isLegacyKind, type Mode } from "../core";
import { symbolOf } from "../core/manifest";
import { shortAddress } from "../core/text";
import { saveSetting, useApp, useLive } from "../data/app";
import { db, type Account, type Address, type Wallet } from "../data/db";
import { addressPath, ensureAddress, firstAddress } from "../data/pairing";
import { accountIcon, accountName, accountNetworks } from "../ui/labels";
import { Picker } from "../ui/Picker";
import { Button, Chip, Fact, FullAddress, LABEL, Notice, SelectList, SelectRow, SkeletonRows } from "../ui/primitives";
import { QrCode } from "../ui/QrCode";
import { Sheet, SheetActions } from "../ui/Sheet";
import { TokenIcon } from "../ui/TokenIcon";
import { t } from "../core/i18n";

/** 发放过的地址，最新的在前。 */
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

function AccountLine({ account, addresses }: { account: Account; addresses: readonly Address[] }) {
    const first = firstAddress(addresses, account.id);
    return (
        <>
            <TokenIcon network={accountIcon(account)} size={32} />
            <span className="flex min-w-0 flex-1 flex-col text-left">
                <span className="truncate text-t3 font-medium">{accountName(account)}</span>
                <span className="truncate font-mono text-t1 text-text-3">
                    {first ? shortAddress(first.address) : ""}
                </span>
            </span>
            {first?.verified ? null : (
                <span role="img" aria-label={t("未核对")} className="size-2 shrink-0 rounded-full bg-warn" />
            )}
        </>
    );
}

type View = "main" | "verify" | "list";

interface PanelProps {
    wallet: Wallet;
    account: Account;
    mode: Mode;
    start: View;
    onIndex(index: number | null): void;
    index: string | null;
}

/** 一个账户的收款地址。收款、核对、地址列表在这里切换，不离开浮层。 */
function AddressPanel({ wallet, account, mode, start, index, onIndex }: PanelProps) {
    const { indexKey, latest, issued } = useIssued(account.id);
    const row = issued.find((item) => String(item.index) === index) ?? issued[0];
    const [view, setView] = useState<View>(start);
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
            setError(t("浏览器不允许复制，请手动选择地址。"));
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
            setError(t("无法生成新地址。"));
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
                <Notice>{t("使用新地址后，旧地址仍然可以收款。")}</Notice>
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
            <div className="mx-auto w-full nav:max-w-[297px]">
                {verifying ? (
                    <QrCode value={frame} size={297} label={t("地址核对二维码")} />
                ) : (
                    <QrCode value={row.address} plain size={297} label={t("收款地址二维码")} />
                )}
            </div>
            <div className="flex flex-col gap-s1">
                {/* 高度按地址列表的入口留，换账户时下面的内容不动。 */}
                <div className="flex min-h-11 items-center justify-between gap-s3 nav:min-h-8">
                    <span className="flex min-w-0 items-center gap-s3">
                        <span className={LABEL}>{t("地址")}</span>
                        {account.chain === "bitcoin" && !verifying ? (
                            <button type="button" className="link" onClick={() => setView("list")}>
                                {t("地址列表")}
                            </button>
                        ) : null}
                    </span>
                    <span key={String(row.verified)} className="fade-in">
                        <Chip tone={row.verified ? "ok" : "warn"}>{row.verified ? t("已核对") : t("未核对")}</Chip>
                    </span>
                </div>
                {/* 最长的地址手机上三行，宽屏两行，短地址也留出同样的高度，换账户时下面的内容不跳。 */}
                <FullAddress
                    address={row.address}
                    className="min-h-[calc(3lh+2*var(--s1))] content-start text-t3 @2xl:min-h-[calc(2lh+var(--s1))]"
                />
            </div>
            <dl className="flex flex-col text-t2">
                <Fact label={t("网络")}>{accountNetworks(account.chain, mode)}</Fact>
                {verifying ? (
                    <Fact label={t("路径")} mono>
                        {path}
                    </Fact>
                ) : null}
            </dl>
            {verifying ? (
                <Notice tone="warn">
                    {t("用冷钱包扫这个码，不要发给付款方。显示属于本钱包并且逐段一致，才算通过。")}
                </Notice>
            ) : row.verified ? null : (
                <Notice tone="warn">{t("收款前先在冷钱包核对地址，以冷钱包屏幕为准。")}</Notice>
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

/** 收款是动作，不是页面。从哪里打开，关掉就回到哪里。 */
export default function ReceiveDialog() {
    const { wallet, accounts, addresses, mode } = useApp();
    const [params, setParams] = useSearchParams();
    const { state } = useLocation();
    const list = accounts.filter((account) => !isLegacyKind(account.kind));
    const account = list.find((item) => item.id === params.get("receive")) ?? list[0];
    const selected = account ? list.indexOf(account) : -1;
    const asset = params.get("asset");
    const symbol = asset ? symbolOf(asset) : null;

    function update(change: (next: URLSearchParams) => void) {
        const next = new URLSearchParams(params);
        change(next);
        setParams(next, { replace: true, state });
    }

    function pick(id: string) {
        update((next) => {
            next.set("receive", id);
            next.delete("index");
            next.delete("verify");
        });
    }

    return (
        <Sheet title={symbol ? t("收款 {0}", symbol) : t("收款")} width={760}>
            {wallet && account ? (
                <div className="grid grid-cols-1 items-start gap-s4 @2xl:grid-cols-[15rem_minmax(0,1fr)] @2xl:gap-s6">
                    <div className="min-w-0">
                        <div className="hidden @2xl:block">
                            <SelectList selected={selected}>
                                {list.map((item) => (
                                    <SelectRow
                                        key={item.id}
                                        selected={item.id === account.id}
                                        onClick={() => pick(item.id)}
                                    >
                                        <AccountLine account={item} addresses={addresses} />
                                    </SelectRow>
                                ))}
                            </SelectList>
                        </div>
                        <div className="@2xl:hidden">
                            <Picker
                                id="receive-account"
                                label={t("账户")}
                                value={<AccountLine account={account} addresses={addresses} />}
                                items={list.map((item) => ({
                                    key: item.id,
                                    node: <AccountLine account={item} addresses={addresses} />,
                                    selected: item.id === account.id,
                                }))}
                                onPick={pick}
                            />
                        </div>
                    </div>
                    <div className="min-w-0">
                        <AddressPanel
                            key={account.id}
                            wallet={wallet}
                            account={account}
                            mode={mode}
                            start={params.has("verify") ? "verify" : "main"}
                            index={params.get("index")}
                            onIndex={(index) =>
                                update((next) => {
                                    if (index === null) next.delete("index");
                                    else next.set("index", String(index));
                                })
                            }
                        />
                    </div>
                </div>
            ) : (
                <SkeletonRows />
            )}
        </Sheet>
    );
}
