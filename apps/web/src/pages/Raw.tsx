import { useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { chainOf } from "../chains";
import type { RawField } from "../chains/spec";
import { failureText } from "../chains/transfer";
import { networkList, networkName } from "../core";
import { t } from "../core/i18n";
import { shortAddress } from "../core/text";
import { useApp } from "../data/app";
import type { Wallet } from "../data/db";
import { createRaw } from "../data/transfer";
import { buildRows, type AssetRow } from "../gateway/assets";
import { Icon } from "../ui/Icon";
import { accountKind } from "../ui/labels";
import { Picker } from "../ui/Picker";
import { Button, ChildTitle, Empty, Helper, LABEL, Notice, Section, Segmented } from "../ui/primitives";
import { TokenIcon, networkIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";

function RowValue({ row }: { row: AssetRow }) {
    return (
        <>
            <TokenIcon network={networkIcon(row.network)} size={28} />
            <span className="flex min-w-0 flex-col">
                <span className="truncate text-t3 font-medium">
                    {networkName(row.network)} {row.account.kind === "single" ? "" : accountKind(row.account)}
                </span>
                <span className="font-mono text-t1 text-text-3">{shortAddress(row.address)}</span>
            </span>
        </>
    );
}

function Field({ field, value, onChange }: { field: RawField; value: string; onChange(value: string): void }) {
    const common = {
        id: `raw-${field.id}`,
        value,
        spellCheck: false,
        autoComplete: "off",
        onChange: (event: { target: { value: string } }) => onChange(event.target.value),
    };
    return (
        <div className="flex flex-col gap-s2">
            <label htmlFor={common.id} className={LABEL}>
                {field.label}
            </label>
            {field.multiline ? (
                <textarea {...common} rows={6} className="field h-auto resize-y py-s3 font-mono text-t2" />
            ) : (
                <input {...common} type="text" className="field font-mono text-t2" />
            )}
            {field.hint ? <Helper>{field.hint}</Helper> : null}
        </div>
    );
}

function RawForm({ wallet, row, picker }: { wallet: Wallet; row: AssetRow; picker: ReactNode }) {
    const { accounts } = useApp();
    const navigate = useNavigate();
    const modes = chainOf(row.account.chain).rawModes();
    const [modeId, setModeId] = useState<string | null>(null);
    const mode = modes.find((item) => item.id === modeId) ?? modes[0];
    const [values, setValues] = useState<Record<string, string>>({});
    const [busy, setBusy] = useState(false);
    const [failure, setFailure] = useState("");
    if (!mode) return null;

    async function submit(id: string) {
        setBusy(true);
        setFailure("");
        try {
            const draft = { wallet, accounts, account: row.account, from: row.address, network: row.network };
            const tx = await createRaw(draft, id, values);
            await navigate(`/sign/${encodeURIComponent(tx.id)}`);
        } catch (error) {
            setFailure(failureText(error));
        } finally {
            setBusy(false);
        }
    }

    return (
        <Section aria-label={t("原始请求")} className="gap-s5">
            <Notice tone="warn">
                {t("这里的内容不经过转账表单的检查,签名后无法撤回.只签你看得懂的数据,并逐项核对冷钱包屏幕.")}
            </Notice>
            {picker}
            {modes.length > 1 ? (
                <Segmented
                    label={t("内容")}
                    value={mode.id}
                    onChange={(id) => {
                        setModeId(id);
                        setFailure("");
                    }}
                    options={modes.map((item) => ({ value: item.id, label: item.label }))}
                />
            ) : null}
            {mode.fields.map((field) => (
                <Field
                    key={`${mode.id}-${field.id}`}
                    field={field}
                    value={values[field.id] ?? ""}
                    onChange={(value) => {
                        setValues((current) => ({ ...current, [field.id]: value }));
                        setFailure("");
                    }}
                />
            ))}
            {failure ? <Notice tone="danger">{failure}</Notice> : null}
            <Button variant="primary" size="lg" disabled={busy} onClick={() => void submit(mode.id)}>
                <Icon name="qr" />
                {t("生成二维码")}
            </Button>
        </Section>
    );
}

/** 让手机签转账表单做不到的内容.之后的二维码,核验和广播都和转账相同. */
export default function Raw() {
    const { wallet, accounts, addresses, mode } = useApp();
    const rows = useMemo(() => {
        const all = networkList().map((item) => item.id);
        return buildRows(accounts, addresses, mode, all, new Map());
    }, [accounts, addresses, mode]);
    const [rowId, setRowId] = useState<string | null>(null);
    const row = rows.find((item) => item.id === rowId) ?? rows[0];

    return (
        <>
            <ChildTitle parent={{ to: "/settings", label: t("设置") }}>{t("原始请求")}</ChildTitle>
            {!wallet ? (
                <NoWallet />
            ) : row ? (
                <RawForm
                    key={row.account.chain}
                    wallet={wallet}
                    row={row}
                    picker={
                        <div className="flex flex-col gap-s2">
                            <span className={LABEL}>{t("付款账户")}</span>
                            <Picker
                                id="raw-from"
                                label={t("付款账户")}
                                value={<RowValue row={row} />}
                                items={rows.map((item) => ({ key: item.id, node: <RowValue row={item} /> }))}
                                onPick={setRowId}
                            />
                        </div>
                    }
                />
            ) : (
                <Empty icon="send">{t("没有可以转出的资产.")}</Empty>
            )}
        </>
    );
}
