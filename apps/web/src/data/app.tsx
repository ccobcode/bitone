// 全局状态.钱包,账户,设置都直接来自 IndexedDB,多个标签页由 liveQuery 同步.
import { liveQuery } from "dexie";
import {
    createContext,
    use,
    useCallback,
    useEffect,
    useMemo,
    useState,
    type DependencyList,
    type ReactNode,
} from "react";
import { accountRank, type Mode } from "../core";
import { db, type Account, type Address, type Wallet } from "./db";
import { isPersisted } from "./pairing";
import { t } from "../core/i18n";

export function useLive<T>(query: () => Promise<T>, deps: DependencyList): T | undefined {
    const [value, setValue] = useState<T>();
    useEffect(() => {
        const subscription = liveQuery(query).subscribe({ next: setValue, error: console.error });
        return () => subscription.unsubscribe();
        // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖由调用方传入,调用方按 biome.json 的 useLive 配置检查.
    }, deps);
    return value;
}

export function saveSetting(key: string, value: unknown): Promise<string> {
    return db.settings.put({ key, value });
}

interface Snapshot {
    settings: Map<string, unknown>;
    wallets: Wallet[];
}

interface AppState {
    mode: Mode;
    setMode(mode: Mode): Promise<void>;
    wallets: Wallet[];
    /** 当前模式下选中的钱包. */
    wallet: Wallet | null;
    selectWallet(wallet: Wallet): Promise<void>;
    accounts: Account[];
    addresses: Address[];
    persisted: boolean;
    refreshPersisted(): Promise<void>;
}

const AppContext = createContext<AppState | null>(null);

export function AppProvider({ children }: { children: ReactNode }) {
    const snapshot = useLive<Snapshot>(async () => {
        const [settings, wallets] = await Promise.all([db.settings.toArray(), db.wallets.orderBy("id").toArray()]);
        return { settings: new Map(settings.map((row) => [row.key, row.value])), wallets };
    }, []);

    const settings = snapshot?.settings;
    const wallets = useMemo(() => snapshot?.wallets ?? [], [snapshot]);
    const mode: Mode = settings?.get("mode") === "testnet" ? "testnet" : "mainnet";

    const inMode = wallets.filter((w) => w.mode === mode);
    const wallet = inMode.find((w) => w.id === settings?.get(`wallet:${mode}`)) ?? inMode[0] ?? null;
    const walletId = wallet?.id;

    const data = useLive(async () => {
        if (!walletId) return null;
        const accounts = await db.accounts.where("walletId").equals(walletId).toArray();
        // 主键按路径字符串排序,TRON 会排到 Bitcoin 前面.
        accounts.sort((a, b) => accountRank(a.path, mode) - accountRank(b.path, mode));
        const addresses = await db.addresses
            .where("accountId")
            .anyOf(accounts.map((account) => account.id))
            .toArray();
        return { walletId, accounts, addresses };
    }, [walletId, mode]);
    // 切换钱包时查询结果晚一拍到,不能把上一个钱包的账户配上新钱包的指纹.
    const current = data?.walletId === walletId ? data : null;

    const [persisted, setPersisted] = useState(false);
    const refreshPersisted = useCallback(async () => setPersisted(await isPersisted()), []);
    useEffect(() => void refreshPersisted(), [refreshPersisted]);

    const setMode = useCallback(async (next: Mode) => void (await saveSetting("mode", next)), []);
    const selectWallet = useCallback(async (next: Wallet) => {
        await db.settings.bulkPut([
            { key: "mode", value: next.mode },
            { key: `wallet:${next.mode}`, value: next.id },
        ]);
    }, []);

    const value = useMemo(
        () => ({
            mode,
            setMode,
            wallets,
            wallet,
            selectWallet,
            accounts: current?.accounts ?? [],
            addresses: current?.addresses ?? [],
            persisted,
            refreshPersisted,
        }),
        [mode, setMode, wallets, wallet, selectWallet, current, persisted, refreshPersisted],
    );

    // 首次读取完成前不渲染,避免闪过 W0.
    if (!snapshot) return null;
    return <AppContext value={value}>{children}</AppContext>;
}

export function useApp(): AppState {
    const state = use(AppContext);
    if (!state) throw new Error(t("缺少 AppProvider"));
    return state;
}
