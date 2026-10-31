import { useState } from "react";
import { useSearchParams } from "react-router";
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
import { accountIcon } from "../ui/labels";
import {
    Button,
    Empty,
    FullAddress,
    LABEL,
    LinkButton,
    ListRow,
    Notice,
    PageTitle,
    Section,
    SectionTitle,
    Segmented,
} from "../ui/primitives";
import { Sheet, SheetActions, overlayLink, useCloseOverlay } from "../ui/Sheet";
import { TokenIcon } from "../ui/TokenIcon";
import { t } from "../core/i18n";

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
        <Notice tone="warn">
            {t("与「")}
            {name}
            {t("」首尾相同，中间不同，可能是投毒地址。")}
        </Notice>
    );
}

function Editor({ value, contacts }: { value: EditorState; contacts: Contact[] }) {
    const { mode } = useApp();
    const close = useCloseOverlay();
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
            close();
        } catch (error) {
            setFailure(error instanceof ContactError ? error.message : t("保存失败，请重试。"));
        }
    }

    return (
        <Sheet title={form.id ? t("编辑地址") : t("添加地址")}>
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
            {failure ? <Notice tone="danger">{failure}</Notice> : null}
            <SheetActions>
                <Button onClick={close}>{t("取消")}</Button>
                <Button variant="primary" onClick={() => void save()}>
                    {t("保存")}
                </Button>
            </SheetActions>
        </Sheet>
    );
}

/** 添加和编辑联系人。`?edit=new` 是添加，其余是联系人 id。 */
export function ContactDialog() {
    const { mode } = useApp();
    const [params] = useSearchParams();
    const contacts = useLive(async () => (await db.contacts.toArray()).filter((row) => row.mode === mode), [mode]);
    if (!contacts) return null;
    const id = params.get("edit");
    const contact = contacts.find((row) => row.id === id);
    const value = contact
        ? { id: contact.id, name: contact.name, chain: contact.chain, address: contact.address }
        : { name: "", chain: "evm" as const, address: "" };
    return <Editor key={id} value={value} contacts={contacts.filter((row) => !row.pending)} />;
}

function PendingSection({ list, confirmed }: { list: Contact[]; confirmed: Contact[] }) {
    return (
        <Section aria-labelledby="h-pend" className="gap-s2">
            <SectionTitle id="h-pend">{t("待确认")}</SectionTitle>
            <Notice tone="warn">{t("这些联系人来自备份，确认后才能用于转账。")}</Notice>
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
        </Section>
    );
}

export default function Contacts() {
    const { mode } = useApp();
    const contacts = useLive(async () => (await db.contacts.toArray()).filter((row) => row.mode === mode), [mode]);
    const list = contacts ?? [];
    const pending = list.filter((row) => row.pending);
    const confirmed = list.filter((row) => !row.pending);

    return (
        <>
            <PageTitle
                aside={
                    <LinkButton variant="primary" size="sm" className="ml-auto" {...overlayLink("?edit=new")}>
                        {t("添加地址")}
                    </LinkButton>
                }
            >
                {t("地址簿")}
            </PageTitle>
            {pending.length > 0 ? <PendingSection list={pending} confirmed={confirmed} /> : null}

            <Section aria-label={t("联系人")} className="gap-0">
                {confirmed.length > 0 ? (
                    <ul className="stagger">
                        {confirmed.map((contact) => (
                            <ListRow
                                key={contact.id}
                                icon={<TokenIcon network={accountIcon(chainView(contact.chain))} size={32} />}
                                title={contact.name}
                                sub={<span className="font-mono">{shortAddress(contact.address)}</span>}
                                actions={
                                    <>
                                        <LinkButton
                                            to={`/send?contact=${encodeURIComponent(contact.id)}`}
                                            variant="ghost"
                                            size="sm"
                                        >
                                            {t("转账")}
                                        </LinkButton>
                                        <LinkButton
                                            {...overlayLink(`?edit=${encodeURIComponent(contact.id)}`)}
                                            variant="ghost"
                                            size="sm"
                                        >
                                            {t("编辑")}
                                        </LinkButton>
                                        <Button
                                            variant="ghost"
                                            size="sm"
                                            className="text-danger hover:text-danger"
                                            onClick={() => void removeContact(contact.id)}
                                        >
                                            {t("删除")}
                                        </Button>
                                    </>
                                }
                            />
                        ))}
                    </ul>
                ) : (
                    <Empty icon="contacts">
                        {pending.length > 0 ? t("确认后的联系人显示在这里。") : t("地址簿还是空的。")}
                    </Empty>
                )}
            </Section>
        </>
    );
}
