// 网关连接状态、余额、价格和交易跟踪。余额和链上记录在本地库里，快照和推送都写入本地，界面只读本地。
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { liveQuery } from "dexie";
import { createContext, use, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { assetsOf } from "../core/manifest";
import { useApp, useLive } from "../data/app";
import { db } from "../data/db";
import { discoverAccount } from "../data/discovery";
import { applyChainUpdate, hasOpenEntries, loadOlder as fetchOlder, syncFeed, type FeedFailure } from "../data/feed";
import { historyStreams, type HistoryStream } from "./history";
import { onSync } from "../data/sync";
import { sweepExpired } from "../data/txs";
import { accountNetworks, buildTargets, type Target } from "./assets";
import { fetchPrices, fetchVersion, parsePrices, parseUpdate, type Balance, type PriceMap } from "./client";
import { BalanceSocket, type SocketStatus } from "./socket";
import { applyPush, pollOpenTxs, watchedTxs } from "./tracker";
import { t } from "../core/i18n";

/** 断线时的 HTTP 轮询间隔。 */
export const POLL_INTERVAL = 15_000;
const FEED_OPEN_INTERVAL = 60_000;
/** 连接正常时也要轮询，推送只带状态，被替换要靠 HTTP 查 nonce 和 UTXO。 */
const LIVE_POLL_INTERVAL = 60_000;
/** 检查本地到期和该不该轮询的间隔。 */
const TICK_INTERVAL = 5000;

interface GatewayState {
    /** 网关配置了上游的网络。 */
    supported: string[];
    targets: Target[];
    balances: Map<string, Balance[]>;
    failures: FeedFailure[];
    /** 全部查询都还没有结果。 */
    loading: boolean;
    /** HTTP 不可用，或者全部网络都查询失败。 */
    down: boolean;
    httpOk: boolean;
    socket: SocketStatus;
    retryAt: number | null;
    lastUpdated: number;
    latency: number | null;
    /** 主网的美元价格。测试网和没有配置价格源时为空。 */
    prices: PriceMap | null;
    /** 地址发现没有完成，Bitcoin 余额可能不全。 */
    discoveryFailed: boolean;
    reconnect(): void;
    /** 没有索引服务的网络，查不到链上历史。 */
    noHistory: string[];
    /** 要同步链上历史的地址。 */
    feedStreams: HistoryStream[];
    syncing: boolean;
    /** 这次同步失败的网络。 */
    syncFailed: string[];
    /** 往前翻一页交易记录。 */
    loadOlder(streams: readonly HistoryStream[]): Promise<void>;
}

const GatewayContext = createContext<GatewayState | null>(null);

function socketUrl(): string {
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    return `${scheme}//${location.host}/v1/ws`;
}

async function timedVersion() {
    const started = performance.now();
    const version = await fetchVersion();
    return { ...version, latency: Math.round(performance.now() - started) };
}

/** 手续费、nonce 这类随链变化的查询。地址发现不在内，重连时重跑发现会写入地址，又引发一轮查询。 */
const chainData = { queryKey: ["chain"] };

export function GatewayProvider({ children }: { children: ReactNode }) {
    const { accounts, addresses, mode } = useApp();
    const client = useQueryClient();
    const [socketState, setSocketState] = useState<{ status: SocketStatus; retryAt: number | null }>({
        status: "connecting",
        retryAt: null,
    });
    const live = socketState.status === "live";

    const version = useQuery({
        queryKey: ["version"],
        queryFn: timedVersion,
        retry: 1,
        // 顶栏的延迟靠它更新。
        refetchInterval: POLL_INTERVAL,
    });
    const supported = useMemo(() => version.data?.networks ?? [], [version.data]);
    const noHistory = useMemo(() => version.data?.noHistory ?? [], [version.data]);
    const targets = useMemo(
        () => buildTargets(accounts, addresses, mode, supported),
        [accounts, addresses, mode, supported],
    );

    const prices = useQuery({
        queryKey: ["prices"],
        queryFn: fetchPrices,
        retry: 1,
        refetchInterval: live ? false : POLL_INTERVAL,
    });

    // 地址发现只用于 Bitcoin。派生出的新地址写入 addresses，余额和订阅随之更新。
    const walletId = accounts[0]?.walletId;
    const cursors = useLive(() => db.feedCursors.toArray(), []);
    const streams = useMemo(() => historyStreams(walletId ?? "", targets, cursors ?? []), [walletId, targets, cursors]);
    const feedStreams = useMemo(
        () => streams.filter((stream) => !noHistory.includes(stream.network)),
        [streams, noHistory],
    );
    const feedTarget = useRef({ walletId, streams, noHistory });
    const feedRun = useRef({ running: false, again: false });
    const [feed, setFeed] = useState({
        syncing: false,
        failed: [] as string[],
        failures: [] as FeedFailure[],
        updatedAt: 0,
    });
    const stored = useLive(() => db.balances.toArray(), []);
    const balances = useMemo(() => {
        const grouped = new Map<string, Balance[]>();
        for (const { network, address, asset, amount } of stored ?? []) {
            const rows = grouped.get(network) ?? [];
            rows.push({ address, asset, amount: BigInt(amount) });
            grouped.set(network, rows);
        }
        return grouped;
    }, [stored]);

    // 同一时间只跑一轮，期间的请求合并成跑完后再来一轮。
    const syncNow = useCallback(async () => {
        const run = feedRun.current;
        if (run.running) {
            run.again = true;
            return;
        }
        run.running = true;
        setFeed((previous) => ({ ...previous, syncing: true }));
        let result: { failed: string[]; failures: FeedFailure[] } = { failed: [], failures: [] };
        do {
            run.again = false;
            const target = feedTarget.current;
            if (target.walletId) {
                result = await syncFeed(target.walletId, target.streams, assetsOf, target.noHistory);
            }
        } while (run.again);
        run.running = false;
        setFeed({ syncing: false, updatedAt: Date.now(), ...result });
    }, []);
    const discovery = useQuery({
        queryKey: ["discovery", walletId, supported.join(",")],
        queryFn: async () => {
            for (const account of accounts) {
                if (account.chain !== "bitcoin") continue;
                for (const network of accountNetworks(account, mode, supported))
                    await discoverAccount(account, network, mode);
            }
            return true;
        },
        enabled: Boolean(walletId) && supported.length > 0,
        staleTime: Number.POSITIVE_INFINITY,
        retry: 1,
    });

    const streamsRef = useRef(streams);
    /** 每批地址收到的最后一个高度，用来发现漏掉的推送。 */
    const heights = useRef(new Map<string, number>());
    const socketRef = useRef<BalanceSocket | null>(null);
    const supportedRef = useRef(supported);
    const lastPoll = useRef(0);

    const pollTxs = useCallback(async () => {
        lastPoll.current = Date.now();
        await pollOpenTxs(supportedRef.current);
    }, []);

    useEffect(() => {
        const socket = new BalanceSocket(socketUrl(), {
            onStatus: (status, retryAt) => {
                setSocketState({ status, retryAt });
                // 断线不一定是网关挂了，立刻用 HTTP 确认一次，不等下一轮轮询。
                if (status === "retrying") void client.invalidateQueries({ queryKey: ["version"] });
            },
            // 先订阅再取快照，这样两者之间到账的交易不会丢。
            onReady: () => {
                heights.current.clear();
                void client.invalidateQueries(chainData);
                void pollTxs();
                void syncNow();
            },
            onAddressesWatched: () => void syncNow(),
            onTx: (network, tx) => void applyPush(network, tx),
            onTxWatched: () => void pollTxs(),
            onPrices: (raw) => client.setQueryData(["prices"], parsePrices(raw)),
            onUpdate: (network, addresses, raw) => {
                const update = parseUpdate(raw, addresses);
                const { walletId: wallet } = feedTarget.current;
                if (!update || !wallet) return;
                const key = `${network}|${addresses[0]}`;
                const last = heights.current.get(key);
                heights.current.set(key, update.height);
                void applyChainUpdate(wallet, { network, addresses: [...addresses] }, update);
                // 接不上说明中间漏了推送，交易没取到时网关也会说。两种情况都取一次快照补齐。
                if ((last !== undefined && update.previous !== last) || update.items === null) void syncNow();
            },
        });
        socket.setTargets(streamsRef.current);
        socket.start();
        socket.watchPrices();
        socketRef.current = socket;
        return () => {
            socket.stop();
            socketRef.current = null;
        };
    }, [client, pollTxs, syncNow]);

    useEffect(() => {
        streamsRef.current = streams;
        socketRef.current?.setTargets(streams);
    }, [streams]);

    useEffect(() => {
        supportedRef.current = supported;
    }, [supported]);

    // 未完成的交易都订阅 tx 频道，包括另一个模式和另一个钱包的。
    useEffect(() => {
        const query = liveQuery(() => watchedTxs(Date.now()));
        const subscription = query.subscribe({
            next: (rows) => {
                const list = rows.flatMap((tx) =>
                    tx.hash && supported.includes(tx.network) ? [{ network: tx.network, hash: tx.hash }] : [],
                );
                socketRef.current?.watchTxs(list);
            },
            error: () => undefined,
        });
        return () => subscription.unsubscribe();
    }, [supported]);

    // 本地到期每隔几秒检查一次。轮询在断线时 15 秒一次，连接正常时只用来发现替换。
    useEffect(() => {
        const timer = setInterval(() => {
            void sweepExpired();
            if (Date.now() - lastPoll.current >= (live ? LIVE_POLL_INTERVAL : POLL_INTERVAL)) void pollTxs();
        }, TICK_INTERVAL);
        return () => clearInterval(timer);
    }, [live, pollTxs]);

    // 交易状态变了，这个标签页和其他标签页缓存的链上数据都要重新查询。
    useEffect(
        () =>
            onSync(() => {
                void client.invalidateQueries(chainData);
            }),
        [client],
    );

    const reconnect = useCallback(() => {
        socketRef.current?.reconnect();
        void client.invalidateQueries();
    }, [client]);

    const failures = feed.failures.filter((failure) => targets.some((target) => target.network === failure.network));
    const everyNetworkFailed = targets.length > 0 && failures.length === targets.length;
    // 全部网络都查询失败也算不可用，版本接口正常不代表上游正常。
    const httpOk = version.isSuccess && !everyNetworkFailed;

    useEffect(() => {
        feedTarget.current = { walletId, streams, noHistory };
    }, [walletId, streams, noHistory]);

    // 地址列表换了引用不算变化，换钱包或地址数量变了才同步。
    const feedKey = [walletId, ...streams.map((stream) => `${stream.network}:${stream.addresses.length}`)].join();
    useEffect(() => {
        if (feedKey) void syncNow();
    }, [feedKey, syncNow]);

    // 推送断开时靠轮询，查询失败的网络也要继续重试，不能等推送。
    useEffect(() => {
        if (live && feed.failures.length === 0) return;
        const timer = setInterval(() => void syncNow(), POLL_INTERVAL);
        return () => clearInterval(timer);
    }, [live, feed.failures.length, syncNow]);

    // 确认中的记录没有余额变化也要更新状态，全部有定论后就不再请求。
    useEffect(() => {
        const timer = setInterval(() => {
            const { walletId: id } = feedTarget.current;
            if (!id) return;
            void hasOpenEntries(id).then((open) => {
                if (open) void syncNow();
            });
        }, FEED_OPEN_INTERVAL);
        return () => clearInterval(timer);
    }, [syncNow]);

    const value: GatewayState = {
        supported,
        targets,
        balances,
        failures,
        loading: version.isPending || (feed.updatedAt === 0 && (stored ?? []).length === 0),
        down: !version.isPending && !httpOk,
        httpOk,
        socket: socketState.status,
        retryAt: socketState.retryAt,
        lastUpdated: feed.updatedAt,
        latency: version.data?.latency ?? null,
        prices: prices.data ?? null,
        discoveryFailed: discovery.isError,
        reconnect,
        noHistory,
        feedStreams,
        syncing: feed.syncing,
        syncFailed: feed.failed,
        loadOlder: async (list) => {
            if (walletId) await fetchOlder(walletId, list);
        },
    };
    return <GatewayContext value={value}>{children}</GatewayContext>;
}

export function useGateway(): GatewayState {
    const state = use(GatewayContext);
    if (!state) throw new Error(t("缺少 GatewayProvider"));
    return state;
}
