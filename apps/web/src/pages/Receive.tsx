import { useMemo, useState } from "react";
import { Navigate, useNavigate, useSearchParams } from "react-router";
import { addressFrame, isLegacyKind, type Mode } from "../core";
import { shortAddress } from "../core/text";
import { saveSetting, useApp, useLive } from "../data/app";
import { db, type Account, type Wallet } from "../data/db";
import { addressPath, ensureAddress } from "../data/pairing";
import { Icon } from "../ui/Icon";
import { accountIcon, accountKind, accountNetworks, accountTitle } from "../ui/labels";
import {
    Alert,
    Button,
    Chip,
    FullAddress,
    Helper,
    LinkButton,
    PageTitle,
    Section,
    SectionTitle,
    SelectList,
    SelectRow,
    SkeletonRows,
} from "../ui/primitives";
import { QrCode } from "../ui/QrCode";
import { TokenIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";
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

function AddressPanel({ wallet, account, mode }: { wallet: Wallet; account: Account; mode: Mode }) {
    const [params, setParams] = useSearchParams();
    const { indexKey, latest, issued } = useIssued(account.id);
    const row = issued.find((item) => String(item.index) === params.get("index")) ?? issued[0];
    const [copied, setCopied] = useState(false);
    const [checking, setChecking] = useState(false);
    const [error, setError] = useState("");

    const path = row ? addressPath(account, row) : "";
    const frame = useMemo(
        () =>
            row
                ? addressFrame({
                      fingerprint: wallet.fingerprint,
                      network: account.network,
                      path,
                      address: row.address,
                  })
                : "",
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
            setParams({ account: account.id }, { replace: true });
        } catch {
            setError(t("无法生成新地址。"));
        }
    }

    return (
        <Section aria-labelledby="h-addr" className="col-span-12 @split:col-span-8">
            {/* 只有 Bitcoin 有历史图标，高度按图标留，换账户时二维码不动。 */}
            <div className="flex min-h-11 items-center justify-between gap-s3 nav:min-h-8">
                <SectionTitle id="h-addr">{t("收款地址")}</SectionTitle>
                <div className="flex items-center gap-s2">
                    {row ? (
                        <span key={String(row.verified)} className="fade-in">
                            <Chip tone={row.verified ? "ok" : "warn"}>{row.verified ? t("已核对") : t("未核对")}</Chip>
                        </span>
                    ) : null}
                    {account.chain === "bitcoin" && row ? (
                        <LinkButton
                            to={`/receive/addresses?account=${encodeURIComponent(account.id)}&index=${row.index}`}
                            variant="ghost"
                            size="iconSm"
                            aria-label={t("地址列表")}
                            title={t("地址列表")}
                        >
                            <Icon name="history" />
                        </LinkButton>
                    ) : null}
                </div>
            </div>
            {row ? (
                <>
                    <Section className="flex-row flex-wrap items-start gap-x-s8 gap-y-s5">
                        <div className="flex w-full max-w-[297px] flex-col gap-s2">
                            {checking ? (
                                <QrCode value={frame} size={297} label={t("地址核对二维码")} />
                            ) : (
                                <QrCode value={row.address} plain size={297} label={t("收款地址二维码")} />
                            )}
                            <p className="text-t2 text-text-3">
                                {checking ? t("用冷钱包扫这个码，不要发给付款方。") : t("付款方扫这个码。")}
                            </p>
                        </div>
                        <div className="flex min-w-0 flex-[1_1_240px] flex-col gap-s4">
                            <div className="flex flex-col gap-s2">
                                <span className="text-t2 text-text-3">{t("地址")}</span>
                                {/* 最长的地址占四行，短地址也留出同样的高度，换账户时下面的内容不跳。 */}
                                <FullAddress
                                    address={row.address}
                                    className="min-h-[calc(4lh+3*var(--s1))] content-start text-t4"
                                />
                            </div>
                            <div className="flex flex-col gap-s1">
                                <span className="text-t2 text-text-3">{t("网络")}</span>
                                <span className="text-t2">{accountNetworks(account.chain, mode)}</span>
                            </div>
                            <div className="flex flex-col gap-s1">
                                <span className="text-t2 text-text-3">{t("路径")}</span>
                                <span className="font-mono text-t1 text-text-2">{path}</span>
                            </div>
                        </div>
                    </Section>
                    {error ? <Alert>{error}</Alert> : null}
                    <div className="flex flex-wrap gap-s2 *:flex-auto @split:justify-end @split:*:flex-none">
                        {account.chain === "bitcoin" ? (
                            <Button onClick={() => void next()}>{t("使用新地址")}</Button>
                        ) : null}
                        <Button onClick={() => void copy(row.address)}>{copied ? t("已复制") : t("复制地址")}</Button>
                        {row.verified ? null : (
                            <Button
                                onClick={() => void db.addresses.update([account.id, 0, row.index], { verified: true })}
                            >
                                {t("标为已核对")}
                            </Button>
                        )}
                        <Button variant="primary" onClick={() => setChecking(!checking)}>
                            {checking ? t("收款二维码") : t("冷钱包核对")}
                        </Button>
                    </div>
                    {account.chain === "bitcoin" ? <Helper>{t("使用新地址后，旧地址仍然可以收款。")}</Helper> : null}
                </>
            ) : (
                <SkeletonRows count={2} />
            )}
        </Section>
    );
}

/** W7b 发放过的地址。点一行回到收款页显示那个地址，序号不变。 */
export function ReceiveAddresses() {
    const { accounts } = useApp();
    const [params] = useSearchParams();
    const navigate = useNavigate();
    const account = accounts.find((item) => item.id === params.get("account"));
    const { latest, issued } = useIssued(account?.id ?? "");
    if (!account) return <Navigate to="/receive" replace />;
    const back = `/receive?account=${encodeURIComponent(account.id)}`;
    const shown = params.get("index") ?? String(latest);

    return (
        <>
            <PageTitle>{t("地址列表")}</PageTitle>
            <Section aria-label={t("地址列表")}>
                <SelectList selected={issued.findIndex((item) => String(item.index) === shown)}>
                    {issued.map((item) => (
                        <SelectRow
                            key={item.index}
                            selected={String(item.index) === shown}
                            onClick={() => void navigate(`${back}&index=${item.index}`)}
                        >
                            <span className="min-w-0 flex-1 truncate font-mono text-t2">
                                {shortAddress(item.address)}
                            </span>
                            {item.index === latest ? <Chip tone="accent">{t("最新")}</Chip> : null}
                        </SelectRow>
                    ))}
                </SelectList>
            </Section>
            <div className="flex justify-end">
                <LinkButton to={back}>{t("返回收款")}</LinkButton>
            </div>
        </>
    );
}

export default function Receive() {
    const { wallet, accounts, addresses, mode } = useApp();
    const [params, setParams] = useSearchParams();
    const list = accounts.filter((account) => !isLegacyKind(account.kind));
    const account = list.find((item) => item.id === params.get("account")) ?? list[0];
    const selected = account ? list.indexOf(account) : -1;

    return (
        <>
            <PageTitle>{t("收款")}</PageTitle>
            <Helper>{t("收款前先在冷钱包核对地址，以冷钱包屏幕为准。")}</Helper>
            {wallet && account ? (
                <>
                    <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                        <Section aria-labelledby="h-acc" className="col-span-12 @split:col-span-4">
                            <SectionTitle id="h-acc">{t("账户")}</SectionTitle>
                            <SelectList selected={selected}>
                                {list.map((item) => {
                                    const first = addresses.find(
                                        (a) => a.accountId === item.id && a.change === 0 && a.index === 0,
                                    );
                                    return (
                                        <SelectRow
                                            key={item.id}
                                            selected={item.id === account.id}
                                            onClick={() => setParams({ account: item.id }, { replace: true })}
                                        >
                                            <TokenIcon network={accountIcon(item)} size={32} />
                                            <span className="flex min-w-0 flex-1 flex-col">
                                                <span className="truncate text-t3 font-medium">
                                                    {accountTitle(item)}{" "}
                                                    {item.chain === "bitcoin" ? accountKind(item) : ""}
                                                </span>
                                                <span className="truncate font-mono text-t1 text-text-3">
                                                    {first ? shortAddress(first.address) : ""}
                                                </span>
                                            </span>
                                        </SelectRow>
                                    );
                                })}
                            </SelectList>
                        </Section>
                        <AddressPanel key={account.id} wallet={wallet} account={account} mode={mode} />
                    </div>
                    <Helper>{t("大额收款请在区块浏览器核对入账。")}</Helper>
                </>
            ) : wallet ? (
                <SkeletonRows />
            ) : (
                <NoWallet />
            )}
        </>
    );
}
