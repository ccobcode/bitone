import { useRef, useState } from "react";
import { useNavigate } from "react-router";
import { saveSetting, useApp } from "../data/app";
import {
    BackupError,
    MIN_PASSPHRASE,
    exportBackup,
    importBackup,
    passphraseLongEnough,
    type ImportSummary,
} from "../data/backup";
import { requestPersist } from "../data/pairing";
import { Icon } from "../ui/Icon";
import { dateLabel } from "../ui/labels";
import { Button, Fact, Helper, LABEL, Notice } from "../ui/primitives";
import { Sheet, SheetActions, useCloseOverlay } from "../ui/Sheet";
import { t } from "../core/i18n";

function download(text: string, name: string): void {
    const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = name;
    link.click();
    URL.revokeObjectURL(url);
}

function failureText(error: unknown): string {
    return error instanceof BackupError ? error.message : t("操作失败,没有写入任何数据.");
}

/** 欢迎页和设置页打开的是同一个浮层. */
export function ImportDialog() {
    const { selectWallet, wallet } = useApp();
    const navigate = useNavigate();
    const upload = useRef<HTMLInputElement>(null);
    const [file, setFile] = useState<File | null>(null);
    const [passphrase, setPassphrase] = useState("");
    const [busy, setBusy] = useState(false);
    const [failure, setFailure] = useState("");
    const [done, setDone] = useState<ImportSummary | null>(null);

    async function restore() {
        if (!file) return;
        setBusy(true);
        setFailure("");
        try {
            const summary = await importBackup(await file.text(), passphrase);
            await requestPersist();
            if (summary.first) await selectWallet(summary.first);
            setPassphrase("");
            setDone(summary);
        } catch (error) {
            setFailure(failureText(error));
        } finally {
            setBusy(false);
        }
    }

    if (done) {
        // 等全局状态读到导入的钱包再离开,否则路由守卫看到的还是没有钱包.
        const ready = !done.first || wallet?.id === done.first.id;
        return (
            <Sheet title={t("导入完成")}>
                <dl className="flex flex-col text-t2">
                    <Fact label={t("账户")}>{t("{0} 个", done.accounts)}</Fact>
                    <Fact label={t("联系人")}>{t("{0} 个", done.contacts)}</Fact>
                </dl>
                {done.conflicts > 0 ? (
                    <Notice tone="warn">{t("{0} 个账户与本地公钥不同,已保留本地的.", done.conflicts)}</Notice>
                ) : null}
                <Notice>{t("导入的联系人和账户都要重新确认")}</Notice>
                <SheetActions>
                    <Button variant="primary" disabled={!ready} onClick={() => void navigate("/", { replace: true })}>
                        {t("完成")}
                    </Button>
                </SheetActions>
            </Sheet>
        );
    }

    return (
        <Sheet title={t("导入备份文件")}>
            <input
                ref={upload}
                type="file"
                accept="application/json,.json"
                hidden
                aria-label={t("选择备份文件")}
                onChange={(event) => {
                    const [picked] = event.target.files ?? [];
                    event.target.value = "";
                    if (picked) setFile(picked);
                }}
            />
            <div className="flex flex-col gap-s2">
                <span className={LABEL}>{t("备份文件")}</span>
                <Button className="justify-start" onClick={() => upload.current?.click()}>
                    <Icon name="upload" />
                    <span className="truncate">{file ? file.name : t("选择备份文件")}</span>
                </Button>
            </div>
            <div className="flex flex-col gap-s2">
                <label htmlFor="bakpw-in" className={LABEL}>
                    {t("备份口令")}
                </label>
                <input
                    id="bakpw-in"
                    type="password"
                    autoComplete="off"
                    placeholder={t("至少 {0} 个字符", MIN_PASSPHRASE)}
                    value={passphrase}
                    onChange={(event) => setPassphrase(event.target.value)}
                    className="field text-t3"
                />
            </div>
            {failure ? <Notice tone="danger">{failure}</Notice> : null}
            <SheetActions>
                <Button
                    variant="primary"
                    disabled={!file || !passphraseLongEnough(passphrase) || busy}
                    onClick={() => void restore()}
                >
                    {t("导入")}
                </Button>
            </SheetActions>
        </Sheet>
    );
}

/** 口令输两次.输错的备份解不开,而且要等到恢复时才发现. */
export function ExportDialog() {
    const close = useCloseOverlay();
    const [passphrase, setPassphrase] = useState("");
    const [again, setAgain] = useState("");
    const [busy, setBusy] = useState(false);
    const [failure, setFailure] = useState("");
    const [exported, setExported] = useState(false);
    const ready = passphraseLongEnough(passphrase) && passphrase === again;

    async function save() {
        setBusy(true);
        setFailure("");
        try {
            const text = await exportBackup(passphrase);
            download(text, `bitone-backup-${dateLabel(Date.now())}.json`);
            await saveSetting("lastBackup", Date.now());
            setPassphrase("");
            setAgain("");
            setExported(true);
        } catch (error) {
            setFailure(failureText(error));
        } finally {
            setBusy(false);
        }
    }

    if (exported) {
        return (
            <Sheet title={t("导出加密备份")}>
                <p role="status" className="text-t3 text-text-2">
                    {t("备份已导出.")}
                </p>
                <SheetActions>
                    <Button variant="primary" onClick={close}>
                        {t("完成")}
                    </Button>
                </SheetActions>
            </Sheet>
        );
    }

    return (
        <Sheet title={t("导出加密备份")}>
            <div className="flex flex-col gap-s2">
                <label htmlFor="bakpw" className={LABEL}>
                    {t("备份口令")}
                </label>
                <input
                    id="bakpw"
                    type="password"
                    autoComplete="new-password"
                    placeholder={t("至少 {0} 个字符", MIN_PASSPHRASE)}
                    value={passphrase}
                    onChange={(event) => setPassphrase(event.target.value)}
                    className="field text-t3"
                />
            </div>
            <div className="flex flex-col gap-s2">
                <label htmlFor="bakpw-again" className={LABEL}>
                    {t("再输一次")}
                </label>
                <input
                    id="bakpw-again"
                    type="password"
                    autoComplete="new-password"
                    value={again}
                    onChange={(event) => setAgain(event.target.value)}
                    className="field text-t3"
                />
            </div>
            {again && passphrase !== again ? <Helper tone="text-warn">{t("两次输入的口令不一致.")}</Helper> : null}
            {failure ? <Notice tone="danger">{failure}</Notice> : null}
            <SheetActions>
                <Button variant="primary" disabled={!ready || busy} onClick={() => void save()}>
                    <Icon name="download" />
                    {t("导出")}
                </Button>
            </SheetActions>
        </Sheet>
    );
}
