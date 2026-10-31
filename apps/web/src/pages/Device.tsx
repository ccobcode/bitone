import { useRef, useState } from "react";
import { Link } from "react-router";
import { isLegacyKind, manifestPackageVersion } from "../core";
import { cleanText, shortAddress } from "../core/text";
import { useApp } from "../data/app";
import { accountIcon, accountKind, accountTitle } from "../ui/labels";
import { AnimatedQr } from "../ui/AnimatedQr";
import {
    Alert,
    Button,
    Chip,
    Collapse,
    Empty,
    Fact,
    Helper,
    LinkButton,
    ListHead,
    PageTitle,
    Section,
    SectionTitle,
    SelectList,
    SelectRow,
} from "../ui/primitives";
import { TokenIcon } from "../ui/TokenIcon";
import { t, translate } from "../core/i18n";

const GRID = "grid grid-cols-[minmax(0,1fr)_auto] gap-x-s4 @split:grid-cols-12 @split:gap-x-s6";

function day(time: number): string {
    const date = new Date(time);
    const pad = (value: number) => String(value).padStart(2, "0");
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** 与核心库的 MAX_MESSAGE_BYTES 一致，超限时二维码发不出去。 */
const MAX_PACKAGE = 256 * 1024;

function kib(bytes: number): string {
    return `${Math.ceil(bytes / 1024)} KiB`;
}

/** 清单包由厂商签名，网页端只负责显示，验签和版本检查都在冷钱包。 */
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
            setFailure(t("清单包超过 {0}，不能用二维码传输。", kib(MAX_PACKAGE)));
            return;
        }
        const bytes = new Uint8Array(await file.arrayBuffer());
        try {
            setPack({ name: cleanText(file.name, 64), bytes, version: manifestPackageVersion(bytes) });
        } catch (error) {
            setFailure(t("清单包无效：{0}。", error instanceof Error ? translate(error.message) : t("无法解析")));
        }
    }

    return (
        <Section aria-labelledby="h-man">
            <div className="flex flex-wrap items-start justify-between gap-s4">
                <div className="flex min-w-0 flex-[1_1_320px] flex-col gap-s1">
                    <SectionTitle id="h-man">{t("内置清单")}</SectionTitle>
                    <Helper>{t("冷钱包扫码导入，用内置公钥验签，版本号只增不减。")}</Helper>
                </div>
                <div className="flex gap-s2">
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
            {failure ? <Alert>{failure}</Alert> : null}
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
                    <LinkButton variant="primary" to="/pair">
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
                                {t("配对于")} {day(item.pairedAt)}
                            </span>
                        </SelectRow>
                    ))}
                </SelectList>
            </Section>

            <Section aria-labelledby="h-acc">
                <div className="flex flex-col gap-s1">
                    <SectionTitle id="h-acc">{t("账户")}</SectionTitle>
                    <Helper>{t("这里的核对状态存在浏览器，唯一的依据是冷钱包屏幕。")}</Helper>
                </div>
                {list.length > 0 ? (
                    <div>
                        <ListHead grid={GRID}>
                            <span className="col-span-6">{t("账户")}</span>
                            <span className="col-span-3">{t("类型")}</span>
                            <span className="col-span-3 text-right">{t("核对")}</span>
                        </ListHead>
                        <ul className="stagger">
                            {list.map((account) => {
                                const first = addresses.find(
                                    (a) => a.accountId === account.id && a.change === 0 && a.index === 0,
                                );
                                return (
                                    <li key={account.id} className={`${GRID} row min-h-row items-center py-s2`}>
                                        <span className="flex min-w-0 items-center gap-s3 @split:col-span-6">
                                            <TokenIcon network={accountIcon(account)} size={32} />
                                            <span className="flex min-w-0 flex-col">
                                                <span className="truncate text-t3 font-medium">
                                                    {accountTitle(account)}
                                                </span>
                                                <span className="truncate font-mono text-t1 text-text-3">
                                                    {first ? shortAddress(first.address) : "—"}
                                                </span>
                                            </span>
                                        </span>
                                        <span className="hidden truncate text-t2 text-text-2 @split:col-span-3 @split:block">
                                            {accountKind(account)}
                                        </span>
                                        <span className="flex items-center justify-end gap-s2 @split:col-span-3">
                                            <LinkButton
                                                to={`/receive?account=${encodeURIComponent(account.id)}`}
                                                variant="ghost"
                                                size="sm"
                                                className="row-actions hidden @split:inline-flex"
                                            >
                                                {t("核对")}
                                            </LinkButton>
                                            <Chip tone={first?.verified ? "ok" : "warn"}>
                                                {first?.verified ? t("已核对") : t("未核对")}
                                            </Chip>
                                        </span>
                                        {/* 窄屏没有操作列，点整行去核对。 */}
                                        <Link
                                            to={`/receive?account=${encodeURIComponent(account.id)}`}
                                            aria-label={t("核对 {0}", accountTitle(account))}
                                            className="absolute inset-0 @split:hidden"
                                        />
                                    </li>
                                );
                            })}
                        </ul>
                    </div>
                ) : (
                    <Empty icon="noAccount">{t("这个网络下还没有账户。")}</Empty>
                )}
            </Section>

            <ManifestCard />
        </>
    );
}
