import { useState } from "react";
import type { Chain } from "../core";
import { shortAddress } from "../core/text";
import { useApp, useLive } from "../data/app";
import {
    ContactError,
    MAX_CONTACT_NAME,
    confirmContact,
    removeContact,
    saveContact,
    similarContact,
} from "../data/contacts";
import { db, type Contact } from "../data/db";
import { accountIcon, accountTitle } from "../ui/labels";
import {
    Alert,
    Button,
    Collapse,
    Empty,
    FullAddress,
    Helper,
    LABEL,
    LinkButton,
    ListHead,
    PageTitle,
    Section,
    SectionTitle,
    Segmented,
} from "../ui/primitives";
import { TokenIcon } from "../ui/TokenIcon";
import { t } from "../core/i18n";

const GRID = "grid grid-cols-[minmax(0,1fr)_auto] gap-x-s4 @split:grid-cols-12 @split:gap-x-s6";
const CHAINS: { value: Chain; label: string }[] = [
    { value: "bitcoin", label: "Bitcoin" },
    { value: "evm", label: "EVM" },
    { value: "tron", label: "TRON" },
    { value: "solana", label: "Solana" },
];

const chainView = (chain: Chain) => ({ chain, kind: "single" as const });

interface EditorState {
    id?: string;
    name: string;
    chain: Chain;
    address: string;
}

function SimilarWarning({ name }: { name: string }) {
    return (
        <Alert tone="text-warn">
            {t("与「")}
            {name}
            {t("」首尾相同，中间不同，可能是投毒地址。")}
        </Alert>
    );
}

function Editor({ value, contacts, onClose }: { value: EditorState; contacts: Contact[]; onClose(): void }) {
    const { mode } = useApp();
    const [form, setForm] = useState(value);
    const [failure, setFailure] = useState("");
    const similar = similarContact(
        form.address.trim(),
        contacts.filter((contact) => contact.chain === form.chain && contact.id !== form.id),
    );

    async function save() {
        setFailure("");
        try {
            await saveContact({ ...form, mode });
            onClose();
        } catch (error) {
            setFailure(error instanceof ContactError ? error.message : t("保存失败，请重试。"));
        }
    }

    return (
        <Section aria-labelledby="h-edit" className="flex flex-col gap-s4">
            <SectionTitle id="h-edit">{form.id ? t("编辑地址") : t("添加地址")}</SectionTitle>
            <div className="grid grid-cols-1 gap-s4 @split:grid-cols-2">
                <div className="flex min-w-0 flex-col gap-s2">
                    <label htmlFor="contact-name" className={LABEL}>
                        {t("名称")}
                    </label>
                    <input
                        id="contact-name"
                        type="text"
                        maxLength={MAX_CONTACT_NAME}
                        value={form.name}
                        onChange={(event) => setForm({ ...form, name: event.target.value })}
                        className="field text-t3"
                    />
                </div>
                <div className="flex min-w-0 flex-col gap-s2">
                    <span className={LABEL}>{t("链")}</span>
                    <Segmented
                        label={t("链")}
                        size="lg"
                        stretch
                        value={form.chain}
                        onChange={(chain) => setForm({ ...form, chain })}
                        options={CHAINS}
                    />
                </div>
            </div>
            <div className="flex flex-col gap-s2">
                <label htmlFor="contact-address" className={LABEL}>
                    {t("地址")}
                </label>
                <input
                    id="contact-address"
                    type="text"
                    spellCheck={false}
                    autoComplete="off"
                    value={form.address}
                    onChange={(event) => setForm({ ...form, address: event.target.value })}
                    className="field font-mono text-t2"
                />
            </div>
            {similar ? <SimilarWarning name={similar.name} /> : null}
            {failure ? <Alert>{failure}</Alert> : null}
            <div className="flex justify-end gap-s2">
                <Button onClick={onClose}>{t("取消")}</Button>
                <Button variant="primary" onClick={() => void save()}>
                    {t("保存")}
                </Button>
            </div>
        </Section>
    );
}

function PendingSection({ list, confirmed }: { list: Contact[]; confirmed: Contact[] }) {
    return (
        <section
            aria-labelledby="h-pend"
            className="tint rise flex flex-col gap-s4 rounded-r3 bg-warn-soft p-s4 @split:p-s6"
        >
            <div className="flex flex-col gap-s1">
                <div className="flex items-baseline gap-s2">
                    <SectionTitle id="h-pend" className="text-warn">
                        {t("待确认")}
                    </SectionTitle>
                    <span className="text-t2 text-warn">
                        {list.length} {t("个")}
                    </span>
                </div>
                <p className="text-t2 text-text-2">{t("这些联系人来自备份，确认后才能用于转账。")}</p>
            </div>
            <ul className="stagger flex flex-col gap-s3">
                {list.map((contact) => {
                    const similar = similarContact(
                        contact.address,
                        confirmed.filter((item) => item.chain === contact.chain),
                    );
                    return (
                        <li key={contact.id} className="flex flex-wrap items-center gap-s3">
                            <TokenIcon network={accountIcon(chainView(contact.chain))} size={32} />
                            <span className="flex min-w-0 flex-[1_1_320px] flex-col gap-s1">
                                <span className="text-t3 font-medium">{contact.name}</span>
                                <FullAddress address={contact.address} className="text-t2" />
                                {similar ? <SimilarWarning name={similar.name} /> : null}
                            </span>
                            <span className="ml-auto flex gap-s2">
                                <Button variant="danger" size="sm" onClick={() => void removeContact(contact.id)}>
                                    {t("删除")}
                                </Button>
                                <Button size="sm" onClick={() => void confirmContact(contact.id)}>
                                    {t("确认")}
                                </Button>
                            </span>
                        </li>
                    );
                })}
            </ul>
        </section>
    );
}

export default function Contacts() {
    const { mode } = useApp();
    const contacts = useLive(async () => (await db.contacts.toArray()).filter((row) => row.mode === mode), [mode]);
    // 收起时内容还在，高度才有东西可以过渡。revision 让每次打开都是新的表单。
    const [editing, setEditing] = useState<EditorState | null>(null);
    const [open, setOpen] = useState(false);
    const [revision, setRevision] = useState(0);
    const list = contacts ?? [];
    const pending = list.filter((row) => row.pending);
    const confirmed = list.filter((row) => !row.pending);

    function edit(value: EditorState) {
        setEditing(value);
        setRevision(revision + 1);
        setOpen(true);
    }

    return (
        <>
            <PageTitle>{t("地址簿")}</PageTitle>
            {/* 折叠块和这一行放在一组，收起时不会多占一段间距。 */}
            <div className="flex flex-col">
                <div className="flex flex-wrap items-center justify-between gap-s4">
                    <Helper>{t("不统计转账次数，名称和历史都不是可信依据。")}</Helper>
                    <Button
                        variant="primary"
                        className="ml-auto"
                        onClick={() => edit({ name: "", chain: "evm", address: "" })}
                    >
                        {t("添加地址")}
                    </Button>
                </div>
                <Collapse open={open}>
                    {editing ? (
                        <div className="pt-s4">
                            <Editor
                                key={revision}
                                value={editing}
                                contacts={confirmed}
                                onClose={() => setOpen(false)}
                            />
                        </div>
                    ) : null}
                </Collapse>
            </div>
            {pending.length > 0 ? <PendingSection list={pending} confirmed={confirmed} /> : null}

            <Section aria-label={t("联系人")} className="gap-0">
                {confirmed.length > 0 ? (
                    <div>
                        <ListHead grid={GRID}>
                            <span className="col-span-6">{t("名称")}</span>
                            <span className="col-span-3">{t("网络")}</span>
                            <span className="col-span-3 text-right">{t("操作")}</span>
                        </ListHead>
                        <ul className="stagger">
                            {confirmed.map((contact) => (
                                <li key={contact.id} className={`${GRID} row min-h-row items-center py-s2`}>
                                    <span className="flex min-w-0 items-center gap-s3 @split:col-span-6">
                                        <TokenIcon network={accountIcon(chainView(contact.chain))} size={32} />
                                        <span className="flex min-w-0 flex-col">
                                            <span className="truncate text-t3 font-medium">{contact.name}</span>
                                            <span
                                                className="truncate font-mono text-t1 text-text-3"
                                                title={contact.address}
                                            >
                                                {shortAddress(contact.address)}
                                            </span>
                                        </span>
                                    </span>
                                    <span className="hidden truncate text-t2 text-text-2 @split:col-span-3 @split:block">
                                        {accountTitle(chainView(contact.chain))}
                                    </span>
                                    <span className="row-actions flex items-center justify-end gap-s1 @split:col-span-3">
                                        <LinkButton
                                            to={`/send?contact=${encodeURIComponent(contact.id)}`}
                                            variant="ghost"
                                            size="sm"
                                        >
                                            {t("转账")}
                                        </LinkButton>
                                        <Button
                                            variant="ghost"
                                            size="sm"
                                            onClick={() =>
                                                edit({
                                                    id: contact.id,
                                                    name: contact.name,
                                                    chain: contact.chain,
                                                    address: contact.address,
                                                })
                                            }
                                        >
                                            {t("编辑")}
                                        </Button>
                                        <Button
                                            variant="ghost"
                                            size="sm"
                                            className="text-danger hover:text-danger"
                                            onClick={() => void removeContact(contact.id)}
                                        >
                                            {t("删除")}
                                        </Button>
                                    </span>
                                </li>
                            ))}
                        </ul>
                    </div>
                ) : (
                    <Empty icon="contacts">
                        {pending.length > 0 ? t("确认后的联系人显示在这里。") : t("地址簿还是空的。")}
                    </Empty>
                )}
            </Section>
        </>
    );
}
