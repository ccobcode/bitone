import { chainOf } from "../chains";
import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { PairingReceiver, type Pairing } from "../core";
import { cleanText } from "../core/text";
import { useApp } from "../data/app";
import { requestPersist, savePairing, selectableAccounts, walletId } from "../data/pairing";
import { buzz, CameraPanel, useImageScan, type CameraFailure } from "../ui/Camera";
import { accountIcon, accountKind, accountTitle } from "../ui/labels";
import { Button, Chip, LABEL, Notice } from "../ui/primitives";
import { Icon } from "../ui/Icon";
import { ScanProgress } from "../ui/ScanProgress";
import { Sheet, SheetActions } from "../ui/Sheet";
import { TokenIcon } from "../ui/TokenIcon";
import { t, translate } from "../core/i18n";

function shortKey(key: Uint8Array): string {
    const hex = Array.from(key, (byte) => byte.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 6)}…${hex.slice(-6)}`;
}

/** 连接冷钱包:扫码,选账户.保存后接着核对第一个收款地址,见 docs/04-security.md 第 4 节. */
export default function PairDialog() {
    const { mode, wallet, wallets, selectWallet, refreshPersisted } = useApp();
    const navigate = useNavigate();

    const receiver = useRef<PairingReceiver | null>(null);
    const [progress, setProgress] = useState({ value: 0, frames: 0 });
    const [notice, setNotice] = useState("");
    const [pairing, setPairing] = useState<Pairing | null>(null);

    const [failure, setFailure] = useState<CameraFailure | null>(null);

    const [paths, setPaths] = useState(new Set<string>());
    const [name, setName] = useState("");
    const [saving, setSaving] = useState(false);
    const [saveError, setSaveError] = useState("");
    const [savedId, setSavedId] = useState("");

    // 等全局状态读到新钱包再跳转.否则首次配对时路由守卫看到的还是没有钱包.
    useEffect(() => {
        if (savedId && wallet?.id === savedId) void navigate("/?receive=&verify=1", { replace: true });
    }, [savedId, wallet, navigate]);

    const reset = useCallback(() => {
        receiver.current?.free();
        receiver.current = null;
        setProgress({ value: 0, frames: 0 });
        setPairing(null);
    }, []);

    useEffect(
        () => () => {
            receiver.current?.free();
            receiver.current = null;
        },
        [],
    );

    const feed = useCallback(
        (text: string) => {
            receiver.current ??= new PairingReceiver();
            const current = receiver.current;
            const result = current.feed(text);
            if (result === "repeated") return;
            if (result === "foreign") return setNotice(t("这不是观察钱包二维码."));
            if (result === "invalid") return setNotice(t("这一帧与已收到的数据不一致,可以重新扫描."));
            setNotice("");
            setProgress({ value: current.progress, frames: current.frames });
            if (!current.complete) return;
            let decoded: Pairing;
            try {
                decoded = current.pairing();
            } catch (error) {
                reset();
                setNotice(
                    error instanceof Error && error.message.includes("白名单")
                        ? t("{0},已拒绝.", translate(error.message))
                        : t("二维码数据无法解析,已拒绝."),
                );
                return;
            }
            const id = walletId(decoded.fingerprint, mode);
            setName(wallets.find((item) => item.id === id)?.name ?? t("主钱包"));
            // 两个模式的账户都默认勾选,配对时切换网络不会丢.
            setPaths(new Set(selectableAccounts(decoded).map((account) => account.path)));
            setPairing(decoded);
            buzz();
        },
        [mode, reset, wallets],
    );

    const images = useImageScan(t("上传图片"), feed, setNotice);

    async function submit() {
        if (!pairing) return;
        setSaving(true);
        setSaveError("");
        try {
            const saved = await savePairing(pairing, { name: cleanText(name.trim(), 32) || t("主钱包"), paths, mode });
            await requestPersist();
            await refreshPersisted();
            await selectWallet(saved);
            setSavedId(saved.id);
        } catch {
            setSaveError(t("写入失败,请重试."));
            setSaving(false);
        }
    }

    function toggle(path: string) {
        const next = new Set(paths);
        if (!next.delete(path)) next.add(path);
        setPaths(next);
    }

    const existing = pairing ? wallets.some((w) => w.id === walletId(pairing.fingerprint, mode)) : false;
    const accounts = pairing ? selectableAccounts(pairing, mode) : [];
    const picked = accounts.filter((account) => paths.has(account.path)).length;

    const scanStep = (
        <>
            <CameraPanel
                active={!pairing}
                failure={failure}
                onFailure={setFailure}
                onText={feed}
                onUpload={images.open}
            />
            <ScanProgress value={progress.value} frames={progress.frames} />
            {notice ? <Notice tone="warn">{notice}</Notice> : null}
            <SheetActions>
                {progress.frames > 0 ? <Button onClick={reset}>{t("重新扫描")}</Button> : null}
                {failure ? null : <Button onClick={images.open}>{t("上传图片")}</Button>}
            </SheetActions>
        </>
    );

    const chooseStep = (
        <>
            <div className="flex items-center gap-s3 text-t2">
                <span className="text-text-3">{t("主密钥指纹")}</span>
                <span className="font-mono">{pairing?.fingerprint}</span>
                <span className="ml-auto">
                    <Chip tone="ok">{t("解析完成")}</Chip>
                </span>
            </div>
            <ul className="stagger">
                {accounts.map((account) => (
                    <li key={account.path}>
                        <label className="row flex min-h-row items-center gap-s3 py-s2">
                            <input
                                type="checkbox"
                                checked={paths.has(account.path)}
                                onChange={() => toggle(account.path)}
                                className="peer sr-only"
                            />
                            <span
                                aria-hidden="true"
                                className="flex size-5 shrink-0 items-center justify-center rounded-r1 bg-surface-3 text-on-accent transition-colors peer-checked:bg-accent peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent-text"
                            >
                                {paths.has(account.path) ? <Icon name="check" size={14} draw /> : null}
                            </span>
                            <TokenIcon network={accountIcon(account)} size={32} />
                            <span className="flex min-w-0 flex-1 flex-col">
                                <span className="truncate text-t3 font-medium">{accountTitle(account)}</span>
                                <span className="truncate text-t1 text-text-3">{accountKind(account)}</span>
                            </span>
                            <span className="hidden flex-col items-end whitespace-nowrap @split:flex">
                                <span className="text-t1 text-text-3">{chainOf(account.chain).keyLabel()}</span>
                                <span className="font-mono text-t1 text-text-2">{shortKey(account.key)}</span>
                            </span>
                        </label>
                    </li>
                ))}
            </ul>
            <div className="flex min-w-0 flex-col gap-s2">
                <label htmlFor="wallet-name" className={LABEL}>
                    {t("钱包名称")}
                </label>
                <input
                    id="wallet-name"
                    type="text"
                    maxLength={32}
                    value={name}
                    disabled={existing}
                    onChange={(event) => setName(event.target.value)}
                    className="field text-t3"
                />
            </div>
            {existing ? (
                <Notice>
                    {t("这个钱包已配对,导入后补上缺的账户.公钥和手机不同的账户改用手机的公钥,地址要重新核对.")}
                </Notice>
            ) : null}
            {saveError ? <Notice tone="danger">{saveError}</Notice> : null}
            <SheetActions>
                <Button onClick={reset}>{t("重新扫描")}</Button>
                <Button variant="primary" disabled={picked === 0 || saving} onClick={() => void submit()}>
                    {t("导入")}
                </Button>
            </SheetActions>
        </>
    );

    return (
        <Sheet title={pairing ? t("选择账户") : t("连接冷钱包")}>
            {images.element}
            {pairing ? chooseStep : scanStep}
        </Sheet>
    );
}
