import { useRef, useState } from "react";
import { isLegacyKind, manifestPackageVersion } from "../core";
import { cleanText, shortAddress } from "../core/text";
import { useApp } from "../data/app";
import { firstAddress } from "../data/pairing";
import { accountIcon, accountName, accountTitle, dateLabel } from "../ui/labels";
import { AnimatedQr } from "../ui/AnimatedQr";
import {
    Button,
    Chip,
    Collapse,
    Empty,
    Fact,
    Helper,
    LinkButton,
    ListRow,
    Notice,
    PageTitle,
    Section,
    SectionTitle,
    SelectList,
    SelectRow,
} from "../ui/primitives";
import { overlayLink, receiveLink } from "../ui/Sheet";
import { TokenIcon } from "../ui/TokenIcon";
import { t, translate } from "../core/i18n";

/** 与核心库的 MAX_MESSAGE_BYTES 一致,超限时二维码发不出去. */
const MAX_PACKAGE = 256 * 1024;

function kib(bytes: number): string {
    return `${Math.ceil(bytes / 1024)} KiB`;
}

/** 清单包由厂商签名,网页端只负责显示,验签和版本检查都在冷钱包. */
function ManifestCard() {
    const input = useRef<HTMLInputElement>(null);
    const [pack, setPack] = useState<{ name: string; bytes: Uint8Array; version: number } | null>(null);
    const [shown, setShown] = useState(false);
    const [failure, setFailure] = useState("");

    async function read(file: File) {
        setFailure("");
        setShown(false);
        setPack(null);
        if (file.size > MAX_PACKAGE) {
            setFailure(t("清单包超过 {0},不能用二维码传输.", kib(MAX_PACKAGE)));
            return;
        }
        const bytes = new Uint8Array(await file.arrayBuffer());
        try {
            setPack({ name: cleanText(file.name, 64), bytes, version: manifestPackageVersion(bytes) });
        } catch (error) {
            setFailure(t("清单包无效:{0}.", error instanceof Error ? translate(error.message) : t("无法解析")));
        }
    }

    return (
        <Section aria-labelledby="h-man">
            <div className="flex flex-wrap items-center justify-between gap-s4">
                <div className="flex min-w-0 flex-[1_1_16rem] flex-col gap-s1">
                    <SectionTitle id="h-man">{t("内置清单")}</SectionTitle>
                    <Helper>{t("冷钱包靠它识别代币,更新用厂商签名的清单包.")}</Helper>
                </div>
                <div className="flex grow gap-s2 *:flex-1 nav:grow-0">
                    <Button onClick={() => input.current?.click()}>{t("选择清单包")}</Button>
                    {pack ? (
                        <Button variant="primary" aria-expanded={shown} onClick={() => setShown(!shown)}>
                            {shown ? t("收起清单包") : t("显示清单包")}
                        </Button>
                    ) : null}
                </div>
            </div>
            <input
                ref={input}
                type="file"
                hidden
                aria-label={t("选择清单包文件")}
                onChange={(event) => {
                    const [file] = event.target.files ?? [];
                    event.target.value = "";
                    if (file) void read(file);
                }}
            />
            {pack ? (
                <dl className="rise flex flex-col text-t2">
                    <Fact label={t("清单包")}>{pack.name}</Fact>
                    <Fact label={t("版本")}>v{pack.version}.0.0</Fact>
                    <Fact label={t("大小")}>{kib(pack.bytes.length)}</Fact>
                </dl>
            ) : null}
            {failure ? <Notice tone="danger">{failure}</Notice> : null}
            <Collapse open={Boolean(pack && shown)}>
                {pack && shown ? (
                    <AnimatedQr
                        message={pack.bytes}
                        type="manifest"
                        size={297}
                        label={t("内置清单包动态二维码")}
                        onError={setFailure}
                    />
                ) : null}
            </Collapse>
        </Section>
    );
}

export default function Device() {
    const { wallets, wallet, mode, accounts, addresses, selectWallet } = useApp();
    const list = accounts.filter((account) => !isLegacyKind(account.kind));
    const paired = wallets.filter((item) => item.mode === mode);

    return (
        <>
            <PageTitle>{t("冷钱包")}</PageTitle>

            <Section aria-labelledby="h-wal">
                <div className="flex flex-wrap items-center justify-between gap-s4">
                    <SectionTitle id="h-wal">{t("已配对")}</SectionTitle>
                    <LinkButton variant="primary" {...overlayLink("?connect=1")}>
                        {t("配对新钱包")}
                    </LinkButton>
                </div>
                <SelectList selected={paired.findIndex((item) => item.id === wallet?.id)}>
                    {paired.map((item) => (
                        <SelectRow
                            key={item.id}
                            selected={item.id === wallet?.id}
                            onClick={() => void selectWallet(item)}
                        >
                            <span className="flex min-w-0 flex-1 flex-col">
                                <span className="truncate text-t3 font-medium">{item.name}</span>
                                <span className="font-mono text-t1 text-text-3">{item.fingerprint}</span>
                            </span>
                            <span className="hidden text-t2 text-text-3 @split:inline">
                                {t("配对于")} {dateLabel(item.pairedAt)}
                            </span>
                        </SelectRow>
                    ))}
                </SelectList>
            </Section>

            <Section aria-labelledby="h-acc" className="gap-s2">
                <SectionTitle id="h-acc">{t("账户")}</SectionTitle>
                {list.length > 0 ? (
                    <ul className="stagger">
                        {list.map((account) => {
                            const first = firstAddress(addresses, account.id);
                            return (
                                <ListRow
                                    key={account.id}
                                    link={receiveLink(account.id, { verify: !first?.verified })}
                                    label={first?.verified ? t("收款") : t("核对 {0}", accountTitle(account))}
                                    icon={<TokenIcon network={accountIcon(account)} size={32} />}
                                    title={accountName(account)}
                                    sub={<span className="font-mono">{first ? shortAddress(first.address) : "—"}</span>}
                                    value={
                                        <Chip tone={first?.verified ? "ok" : "warn"}>
                                            {first?.verified ? t("已核对") : t("未核对")}
                                        </Chip>
                                    }
                                />
                            );
                        })}
                    </ul>
                ) : (
                    <Empty icon="noAccount">{t("这个网络下还没有账户.")}</Empty>
                )}
            </Section>

            <ManifestCard />
        </>
    );
}
