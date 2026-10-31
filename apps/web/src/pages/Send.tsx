import { useMemo, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { networkName, validateAddress, type Review } from "../core";
import { formatUnits, formatUsd, parseUnits, toInput, usdCents } from "../core/amount";
import { lookupAsset, priceKey } from "../core/manifest";
import { cleanText, shortAddress } from "../core/text";
import { useApp, useLive } from "../data/app";
import { checkRecipient, usableContacts } from "../data/contacts";
import { db, type Account, type Wallet } from "../data/db";
import { TransferError, createSolanaNonce, createTransfer } from "../data/transfer";
import { reservationsOf } from "../data/txs";
import { buildRows, inManifest, pendingAmount, type AssetRow, type ManifestRow } from "../gateway/assets";
import { GatewayError, addressKey, type FeeTier } from "../gateway/client";
import { txHref } from "../gateway/history";
import { useGateway } from "../gateway/provider";
import { useQuote } from "../gateway/quote";
import { GatewayDown } from "../ui/GatewayDown";
import { Icon } from "../ui/Icon";
import { accountIcon, accountKind, accountTitle, tierLabel } from "../ui/labels";
import { Picker } from "../ui/Picker";
import {
    Button,
    ChildTitle,
    Chip,
    Collapse,
    Disclosure,
    Empty,
    Fact,
    Helper,
    LABEL,
    Notice,
    Section,
    SectionTitle,
    Segmented,
    SkeletonRows,
} from "../ui/primitives";
import { StateCard } from "../ui/StateCard";
import { AssetIcon, TokenIcon, networkIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";
import { t, translate } from "../core/i18n";

/** 选择框里最多显示的小数位，完整金额在「全部」里。 */
const PICKER_FRACTION = 8;

function failureText(error: unknown): string {
    if (error instanceof TransferError) return translate(error.message);
    if (error instanceof GatewayError) return translate(error.detail) || t("暂时无法生成交易，请稍后重试。");
    return error instanceof Error ? translate(error.message) : t("生成失败，没有写入任何记录。");
}

function AccountValue({ account, address }: { account: Account; address: string }) {
    return (
        <>
            <TokenIcon network={accountIcon(account)} size={28} />
            <span className="flex min-w-0 flex-col">
                <span className="truncate text-t3 font-medium">
                    {accountTitle(account)} {account.kind === "single" ? "" : accountKind(account)}
                </span>
                <span className="font-mono text-t1 text-text-3">{shortAddress(address)}</span>
            </span>
        </>
    );
}

/** 金额旁边的资产胶囊。 */
function AssetPill({ row }: { row: ManifestRow }) {
    return (
        <>
            <AssetIcon entry={row.entry} network={row.network} size={32} />
            <span className="flex min-w-0 flex-col">
                <span className="truncate text-t3 leading-5 font-medium">{row.entry.symbol}</span>
                <span className="truncate text-t1 text-text-3">{networkName(row.network)}</span>
            </span>
        </>
    );
}

function AssetValue({ row, available }: { row: ManifestRow; available: string }) {
    return (
        <>
            <AssetIcon entry={row.entry} network={row.network} size={28} />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="flex items-center gap-s2">
                    <span className="text-t3 font-medium">{row.entry.symbol}</span>
                    <span className="text-t1 text-text-3">
                        {networkName(row.network)} {row.account.kind === "single" ? "" : accountKind(row.account)}
                    </span>
                </span>
                <span className="text-t1 text-text-3">
                    {t("可用")} {available}
                </span>
            </span>
        </>
    );
}

interface FormProps {
    wallet: Wallet;
    rows: ManifestRow[];
    first: ManifestRow;
}

function SendForm({ wallet, rows, first }: FormProps) {
    const { accounts, addresses, mode } = useApp();
    const { prices } = useGateway();
    const navigate = useNavigate();
    const [params] = useSearchParams();

    // 从联系人或失败的交易进来时带上预填内容，用户改动后以输入为准。
    const originId = params.get("from");
    const contactId = params.get("contact");
    const origin = useLive(async () => (originId ? ((await db.txs.get(originId)) ?? null) : null), [originId]);
    const preset = useLive(async () => (contactId ? ((await db.contacts.get(contactId)) ?? null) : null), [contactId]);
    const wanted = {
        account: params.get("account") ?? origin?.accountId,
        network: params.get("network") ?? origin?.network,
        asset: params.get("asset") ?? origin?.asset,
    };
    const matched = rows.find(
        (item) =>
            item.account.id === wanted.account &&
            (!wanted.network || item.network === wanted.network) &&
            (!wanted.asset || addressKey(item.asset) === addressKey(wanted.asset)),
    );
    const contactRow = preset ? rows.find((item) => item.account.chain === preset.chain) : undefined;

    const [rowId, setRowId] = useState<string | null>(null);
    const row =
        rows.find((item) => item.id === rowId) ??
        matched ??
        contactRow ??
        rows.find((item) => item.amount > 0n) ??
        first;
    const { account, network, asset, entry } = row;
    const chain = account.chain;

    const [typedTo, setTypedTo] = useState<string | null>(null);
    const [typedAmount, setTypedAmount] = useState<string | null>(null);
    const to = typedTo ?? (preset && !preset.pending ? preset.address : (origin?.to ?? ""));
    const amountText = typedAmount ?? (origin ? toInput(BigInt(origin.amount), entry.decimals) : "");
    const [tier, setTier] = useState<FeeTier>("medium");
    const [advanced, setAdvanced] = useState(false);
    const [gasText, setGasText] = useState<string | null>(null);
    const [limitText, setLimitText] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [failure, setFailure] = useState("");
    const [forbidden, setForbidden] = useState<Review | null>(null);

    const contacts = useLive(() => db.contacts.toArray(), []);
    const txs = useLive(() => db.txs.where("walletId").equals(wallet.id).toArray(), [wallet.id]);
    const reservations = useLive(() => reservationsOf(account.id, network), [account.id, network]);

    /** 默认资产会随余额加载变化。用户开始填写后就锁定，金额不能换到另一个币上。 */
    function keepRow() {
        setRowId((current) => current ?? row.id);
    }

    function pickRow(id: string) {
        setRowId(id);
        setGasText(null);
        setLimitText(null);
        setFailure("");
        setForbidden(null);
    }

    const canonical = useMemo(() => {
        try {
            return to.trim() ? validateAddress(network, to) : null;
        } catch {
            return null;
        }
    }, [network, to]);
    const amount = parseUnits(amountText, entry.decimals);

    const own = useMemo(() => addresses.map((item) => item.address), [addresses]);
    const sameChain = (contacts ?? []).filter((contact) => contact.chain === chain && contact.mode === mode);
    const check = canonical ? checkRecipient(canonical, sameChain, txs ?? [], own) : null;

    const nativeRow = rows.find(
        (item) => item.account.id === account.id && item.network === network && item.asset === "native",
    );
    const spendable = (target: AssetRow | undefined) =>
        target?.loaded ? target.amount - pendingAmount(txs ?? [], account.id, network, target.asset) : null;
    const gasOverride = gasText?.trim() ? parseUnits(gasText, 0) : null;
    const limitOverride = limitText?.trim() ? parseUnits(limitText, 0) : null;

    const base = { wallet, accounts, account, from: row.address, network, asset, entry, tier };
    const quote = useQuote({
        ...base,
        to: canonical,
        amount,
        gasOverride,
        feeLimitOverride: limitOverride,
        balance: spendable(row),
        nativeBalance: spendable(nativeRow),
        reservations: reservations ?? [],
        txs: txs ?? [],
    });

    const native = lookupAsset(network, "native");
    const amountOk = amount !== null && amount > 0n;
    const usd = useMemo(() => {
        const price = prices?.get(priceKey(network, asset));
        if (!price || amount === null) return "";
        const cents = usdCents(amount, entry.decimals, price);
        return cents === null ? "" : `≈ ${formatUsd(cents)}`;
    }, [prices, network, asset, amount, entry.decimals]);

    const shortAmount = (value: bigint, target: ManifestRow) =>
        formatUnits(value, target.entry.decimals, { min: target.entry.minFraction, max: PICKER_FRACTION });
    const available = quote.available === null ? "—" : shortAmount(quote.available, row);
    const blocked =
        !canonical ||
        !amountOk ||
        busy ||
        quote.needsNonceAccount ||
        quote.nonceBusy ||
        quote.held !== null ||
        quote.enough === false ||
        (quote.problem !== null && gasOverride === null) ||
        Boolean(check?.pending);

    function draft() {
        return {
            ...base,
            to: canonical ?? "",
            amount: amount ?? 0n,
            ...(gasOverride !== null ? { gasLimit: gasOverride } : {}),
            ...(limitOverride !== null ? { feeLimit: limitOverride } : {}),
        };
    }

    async function run(work: () => Promise<{ id: string }>) {
        setBusy(true);
        setFailure("");
        setForbidden(null);
        try {
            const tx = await work();
            await navigate(`/sign/${encodeURIComponent(tx.id)}`);
        } catch (error) {
            if (error instanceof TransferError && error.code === "forbidden" && error.review) {
                setForbidden(error.review);
            } else {
                setFailure(failureText(error));
                if (error instanceof TransferError && error.code === "revert") setAdvanced(true);
            }
        } finally {
            setBusy(false);
        }
    }

    const accountItems = [...new Map(rows.map((item) => [item.account.id, item])).values()];

    const selected = quote.tiers?.find((item) => item.tier === tier);

    return (
        <>
            {forbidden ? (
                <StateCard
                    tone="danger"
                    icon="alert"
                    title={t("禁止签名")}
                    body={forbidden.alerts[0] ?? t("这笔交易不能签名，没有生成二维码。")}
                    facts={[
                        { label: t("网络"), value: networkName(network) },
                        { label: t("判定"), value: forbidden.title },
                    ]}
                    actions={<Button onClick={() => setForbidden(null)}>{t("返回修改")}</Button>}
                />
            ) : null}

            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <section aria-label={t("交易信息")} className="card col-span-12 gap-s6 @split:col-span-8">
                    <div className="group flex flex-col gap-s2">
                        <label
                            htmlFor="amt"
                            className="text-t2 text-text-3 transition-colors group-focus-within:text-accent-text"
                        >
                            {t("金额")}
                        </label>
                        {/* 大字号的金额也是输入框，边框和聚焦状态和其他输入框一致。 */}
                        <div className="flex min-w-0 items-center gap-s4 rounded-r2 border border-line-strong bg-surface py-s2 pr-s2 pl-s4 outline-2 outline-transparent transition-colors focus-within:border-accent-text focus-within:outline-accent-soft">
                            <input
                                id="amt"
                                type="text"
                                inputMode="decimal"
                                autoComplete="off"
                                placeholder="0"
                                value={amountText}
                                onChange={(event) => {
                                    keepRow();
                                    setTypedAmount(event.target.value);
                                    setFailure("");
                                }}
                                className="h-14 min-w-0 flex-1 bg-transparent text-t8 font-semibold text-text caret-accent outline-none placeholder:text-text-3"
                            />
                            <Picker
                                id="asset"
                                look="pill"
                                label={t("资产")}
                                value={<AssetPill row={row} />}
                                items={rows.map((item) => ({
                                    key: item.id,
                                    node: (
                                        <AssetValue
                                            row={item}
                                            available={item.loaded ? shortAmount(item.amount, item) : "—"}
                                        />
                                    ),
                                }))}
                                onPick={pickRow}
                            />
                        </div>
                        <div className="flex min-h-8 flex-wrap items-center justify-between gap-x-s4 text-t2 text-text-3">
                            <span key={usd} className="fade-in">
                                {amountText.trim() && amount === null ? t("最多 {0} 位小数", entry.decimals) : usd}
                            </span>
                            <span className="flex items-center gap-s2">
                                {t("可用")} {available} {entry.symbol}
                                <button
                                    type="button"
                                    className="link disabled:cursor-not-allowed disabled:opacity-50"
                                    disabled={quote.max === null}
                                    onClick={() =>
                                        quote.max !== null && setTypedAmount(toInput(quote.max, entry.decimals))
                                    }
                                >
                                    {t("全部")}
                                </button>
                            </span>
                        </div>
                    </div>

                    <div className="flex flex-col gap-s2">
                        <label htmlFor="to" className={LABEL}>
                            {t("收款地址")}
                        </label>
                        <div className="flex gap-s2">
                            <input
                                id="to"
                                type="text"
                                spellCheck={false}
                                autoComplete="off"
                                value={to}
                                onChange={(event) => {
                                    keepRow();
                                    setTypedTo(event.target.value);
                                    setFailure("");
                                }}
                                className="field flex-1 font-mono text-t2"
                            />
                            <Picker
                                id="book"
                                look="compact"
                                value={t("地址簿")}
                                empty={t("地址簿里没有这条链的地址")}
                                items={sameChain.map((contact) => ({
                                    key: contact.id,
                                    disabled: contact.pending,
                                    node: (
                                        <span className="flex min-w-0 flex-1 flex-col">
                                            <span className="flex items-center gap-s2 font-medium">
                                                {contact.name}
                                                {contact.pending ? <Chip tone="warn">{t("待确认")}</Chip> : null}
                                            </span>
                                            <span className="font-mono text-t1 text-text-3">
                                                {shortAddress(contact.address)}
                                            </span>
                                        </span>
                                    ),
                                }))}
                                onPick={(id) => {
                                    const contact = usableContacts(sameChain).find((item) => item.id === id);
                                    if (contact) setTypedTo(contact.address);
                                }}
                            />
                        </div>
                        {to.trim() ? (
                            <div className="rise flex flex-wrap gap-s2">
                                {canonical ? (
                                    <Chip tone="ok">
                                        <Icon name="check" size={12} />
                                        {(chain === "evm" && to.trim() === canonical) || chain === "tron"
                                            ? t("校验和有效")
                                            : t("地址格式有效")}
                                    </Chip>
                                ) : (
                                    <Chip tone="danger">{t("地址无效")}</Chip>
                                )}
                                {check?.contact ? (
                                    <Chip tone="accent">
                                        <Icon name="contacts" size={12} />
                                        {check.contact.name}
                                    </Chip>
                                ) : null}
                                {check?.firstTime ? <Chip tone="warn">{t("首次转账")}</Chip> : null}
                            </div>
                        ) : null}
                        {check?.pending ? (
                            <Helper tone="text-warn">
                                {t("这个地址来自备份，先在地址簿确认「")}
                                {check.pending.name}
                                {t("」才能转账。")}
                            </Helper>
                        ) : null}
                        {check?.similar && canonical ? (
                            <div
                                role="alert"
                                className="rise flex flex-col gap-s2 rounded-r3 bg-warn-soft p-s4 text-t2 text-warn"
                            >
                                <span className="font-medium">
                                    {t("与")}
                                    {check.similar.name}
                                    {t("首尾相同，中间不同，可能是投毒地址。")}
                                </span>
                                <span className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-s3 gap-y-s1 text-text-2">
                                    <span>{t("已有")}</span>
                                    <span className="font-mono [overflow-wrap:anywhere]">{check.similar.address}</span>
                                    <span>{t("本次")}</span>
                                    <span className="font-mono [overflow-wrap:anywhere]">{canonical}</span>
                                </span>
                            </div>
                        ) : null}
                    </div>

                    <div className="flex flex-col gap-s2">
                        <label htmlFor="from" className={LABEL}>
                            {t("付款账户")}
                        </label>
                        <Picker
                            id="from"
                            value={<AccountValue account={account} address={row.address} />}
                            items={accountItems.map((item) => ({
                                key: item.id,
                                node: <AccountValue account={item.account} address={item.address} />,
                            }))}
                            onPick={pickRow}
                        />
                    </div>

                    <div className="flex flex-col gap-s2">
                        <div className="flex items-baseline justify-between gap-s3">
                            <span className={LABEL}>{t("手续费")}</span>
                            <span className="text-t1 text-text-3">{quote.feeTag}</span>
                        </div>
                        {quote.tiers ? (
                            <Segmented<FeeTier>
                                label={t("手续费档位")}
                                size={quote.tiers.some((item) => item.eta) ? "lg" : "md"}
                                stretch
                                value={tier}
                                onChange={setTier}
                                options={quote.tiers.map((item) => ({
                                    value: item.tier,
                                    label: tierLabel(item.tier),
                                    hint: item.eta,
                                }))}
                            />
                        ) : null}
                        {selected ? (
                            <div
                                key={selected.tier}
                                className="fade-in flex flex-wrap items-baseline justify-between gap-x-s4 text-t2"
                            >
                                <span className="[overflow-wrap:anywhere]">{selected.cost}</span>
                                <span className="text-text-3">{selected.rate}</span>
                            </div>
                        ) : null}
                        {quote.rows ? (
                            <dl className="flex flex-col text-t2">
                                {quote.rows.map((item) => (
                                    <Fact key={item.label} label={item.label}>
                                        {item.value}
                                    </Fact>
                                ))}
                            </dl>
                        ) : null}
                        {quote.failed ? <Helper tone="text-danger">{t("手续费暂时不可用。")}</Helper> : null}
                        {quote.problem ? (
                            <Helper tone="text-warn">
                                {chain === "evm"
                                    ? t("估算失败：{0}，可以在高级设置里填写 Gas 上限。", cleanText(quote.problem, 160))
                                    : quote.problem}
                            </Helper>
                        ) : null}
                    </div>

                    <div className="flex flex-col">
                        <Disclosure open={advanced} onToggle={() => setAdvanced(!advanced)}>
                            {t("高级设置")}
                            <span className="ml-auto text-text-3">{quote.advancedLabel}</span>
                        </Disclosure>
                        <Collapse open={advanced}>
                            <div className="grid grid-cols-1 gap-s4 pt-s3 @split:grid-cols-2">
                                {quote.advanced.map((field) => (
                                    <div key={field.id} className="flex min-w-0 flex-col gap-s2">
                                        <label htmlFor={field.id} className={LABEL}>
                                            {field.label}
                                        </label>
                                        <input
                                            id={field.id}
                                            type="text"
                                            inputMode="numeric"
                                            readOnly={!field.editable}
                                            value={
                                                field.id === "adv-gas"
                                                    ? (gasText ?? field.value)
                                                    : field.id === "adv-limit"
                                                      ? (limitText ?? field.value)
                                                      : field.value
                                            }
                                            onChange={(event) =>
                                                field.id === "adv-gas"
                                                    ? setGasText(event.target.value)
                                                    : setLimitText(event.target.value)
                                            }
                                            className="field text-t3"
                                        />
                                    </div>
                                ))}
                            </div>
                        </Collapse>
                    </div>
                </section>

                <aside className="col-span-12 flex min-w-0 flex-col gap-s4 @split:sticky @split:top-[calc(var(--topbar)+var(--safe-top)+var(--s5))] @split:col-span-4">
                    {quote.needsNonceAccount ? (
                        <Section aria-labelledby="h-nonce" className="rise flex flex-col gap-s3">
                            <SectionTitle id="h-nonce">{t("创建 nonce 账户")}</SectionTitle>
                            <Helper>{t("首次 Solana 转账前要先创建，需要少量可赎回的 SOL。")}</Helper>
                            <Button
                                variant="primary"
                                disabled={busy}
                                onClick={() => void run(() => createSolanaNonce(base))}
                            >
                                {t("创建账户")}
                            </Button>
                        </Section>
                    ) : null}

                    <Section aria-labelledby="h-sum" className="flex flex-col gap-s4">
                        <SectionTitle id="h-sum">{t("交易摘要")}</SectionTitle>
                        <dl className="flex flex-col text-t2">
                            <Fact label={t("网络")}>
                                <span className="inline-flex items-center gap-s2">
                                    <TokenIcon network={networkIcon(network)} size={16} />
                                    {networkName(network)}
                                </span>
                            </Fact>
                            <Fact label={t("发送")}>
                                {amountText.trim() || "0"} {entry.symbol}
                            </Fact>
                            <Fact label={t("收款")}>
                                <span className="flex flex-col items-end">
                                    <span className={canonical ? "font-mono" : ""}>
                                        {canonical ? shortAddress(canonical) : "—"}
                                    </span>
                                    <span className="text-t1 text-text-3">
                                        {check?.contact?.name ?? t("不在地址簿")}
                                    </span>
                                </span>
                            </Fact>
                            <Fact label={quote.feeLabel}>
                                <span key={quote.feeText} className="fade-in">
                                    {quote.feeText}
                                </span>
                            </Fact>
                        </dl>
                        <p
                            className={`flex items-center gap-s2 text-t2 ${
                                quote.enough === false ? "text-danger" : quote.enough ? "text-ok" : "text-text-3"
                            }`}
                        >
                            <Icon name={quote.enough === false ? "alert" : "check"} size={16} className="shrink-0" />
                            {quote.enoughText}
                        </p>
                    </Section>

                    {quote.nonceBusy ? (
                        <Notice tone="warn">{t("上一笔 Solana 交易还没有结束，完成后才能转账。")}</Notice>
                    ) : null}
                    {quote.held ? (
                        <Notice tone="warn">
                            <Link to={txHref(quote.held)} className="text-warn">
                                {t("交易")} {quote.held.id} {t("已取消但签名仍然有效，先发替换交易。")}
                            </Link>
                        </Notice>
                    ) : null}
                    {failure ? <Notice tone="danger">{failure}</Notice> : null}
                    <Button
                        variant="primary"
                        size="lg"
                        className="w-full"
                        disabled={blocked}
                        onClick={() => void run(() => createTransfer(draft()))}
                    >
                        <Icon name="qr" />
                        {t("生成二维码")}
                    </Button>
                    <div className="flex flex-col gap-s1 text-center text-t1 text-text-3">
                        <span>{quote.validity}</span>
                        {native && !entry.native ? (
                            <span>
                                {t("手续费用")} {native.symbol} {t("支付")}
                            </span>
                        ) : null}
                    </div>
                </aside>
            </div>
        </>
    );
}

export default function Send() {
    const { wallet, accounts, addresses, mode } = useApp();
    const gateway = useGateway();
    const rows = buildRows(accounts, addresses, mode, gateway.supported, gateway.balances).filter(inManifest);
    const [first] = rows;

    return (
        <>
            <ChildTitle parent={{ to: "/", label: t("资产") }}>{t("发起转账")}</ChildTitle>
            {gateway.down ? <GatewayDown /> : null}
            {!wallet ? (
                <NoWallet />
            ) : first ? (
                <SendForm wallet={wallet} rows={rows} first={first} />
            ) : gateway.down ? null : gateway.loading ? (
                <SkeletonRows />
            ) : (
                <Empty icon="send">{t("没有可以转出的资产。")}</Empty>
            )}
        </>
    );
}
