// 网关连接状态,余额,价格和交易跟踪.余额和链上记录在本地库里,快照和推送都写入本地,界面只读本地.
import { chainOf } from "../chains";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { liveQuery } from "dexie";
import { createContext, use, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { assetsOf } from "../core/manifest";
import { useApp, useLive } from "../data/app";
import { db } from "../data/db";
import { discoverAccount } from "../data/discovery";
import {
    applyChainUpdate,
    drainQueue,
    hasOpenEntries,
    loadOlder as fetchOlder,
    syncFeed,
    type FeedFailure,
} from "../data/feed";
import { incomingNotices, missedNotice, publish } from "../data/notify";
import { historyStreams, type HistoryStream } from "./history";
import { onSync } from "../data/sync";
import { sweepExpired } from "../data/txs";
import { accountNetworks, buildTargets } from "./assets";
import { addressKey, fetchPrices, fetchVersion, parsePrices, parseUpdate, type Balance, type PriceMap } from "./client";
import { BalanceSocket, batchKey, type SocketStatus } from "./socket";
import { applyPush, pollOpenTxs, watchedTxs } from "./tracker";
import { t } from "../core/i18n";

/** 断线时的 HTTP 轮询间隔. */
export const POLL_INTERVAL = 15_000;
/** 一批订阅回复通常同时到,等这么久合成一次快照. */
const SYNC_GATHER = 50;
/** 有未定论的记录时,新块最多这么久触发一次增量. */
const SETTLE_INTERVAL = 30_000;
/** 查询失败的网络的最长重试间隔. */
const MAX_RETRY_INTERVAL = 5 * 60_000;
/** 连接正常时也要轮询,推送只带状态,被替换要靠 HTTP 查 nonce 和 UTXO. */
const LIVE_POLL_INTERVAL = 60_000;
/** 检查本地到期和该不该轮询的间隔. */
const TICK_INTERVAL = 5000;

interface GatewayState {
    /** 网关配置了上游的网络. */
    supported: string[];
    balances: Map<string, Balance[]>;
    failures: FeedFailure[];
    /** 全部查询都还没有结果. */
    loading: boolean;
    /** HTTP 不可用,或者全部网络都查询失败. */
    down: boolean;
    httpOk: boolean;
    socket: SocketStatus;
    retryAt: number | null;
    lastUpdated: number;
    latency: number | null;
    /** 主网的美元价格.测试网和没有配置价格源时为空. */
    prices: PriceMap | null;
    /** 地址发现没有完成,Bitcoin 余额可能不全. */
    discoveryFailed: boolean;
    reconnect(): void;
    /** 没有索引服务的网络,查不到链上历史. */
    noHistory: string[];
    /** 要同步链上历史的地址. */
    feedStreams: HistoryStream[];
    syncing: boolean;
    /** 这次同步失败的网络. */
    syncFailed: string[];
    /** 往前翻一页交易记录. */
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

/** 手续费,nonce 这类随链变化的查询.地址发现不在内,重连时重跑发现会写入地址,又引发一轮查询. */
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
        // 连接正常时推送就是证明,延迟也来自 WebSocket 的心跳.
        refetchInterval: live ? false : POLL_INTERVAL,
    });
    const [latency, setLatency] = useState<number | null>(null);
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

    // 地址发现只用于 Bitcoin.派生出的新地址写入 addresses,余额和订阅随之更新.
    const walletId = accounts[0]?.walletId;
    const cursors = useLive(() => db.feedCursors.toArray(), []);
    const streams = useMemo(() => historyStreams(walletId ?? "", targets, cursors ?? []), [walletId, targets, cursors]);
    const feedStreams = useMemo(
        () => streams.filter((stream) => !noHistory.includes(stream.network)),
        [streams, noHistory],
    );
    const feedTarget = useRef({ walletId, streams, noHistory });
    /** 这个钱包的全部地址和当前价格,判断要不要通知时用. */
    const ownRef = useRef<ReadonlySet<string>>(new Set());
    const pricesRef = useRef<PriceMap | null>(null);
    const feedRun = useRef({
        running: false,
        queue: new Map<string, HistoryStream>(),
        failed: new Set<string>(),
        failures: new Map<string, string>(),
    });
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

    // 同一时间只跑一轮,期间要同步的批次排队,合成下一轮.不传时同步全部批次.
    const syncNow = useCallback(async (list?: readonly HistoryStream[]) => {
        const run = feedRun.current;
        for (const stream of list ?? feedTarget.current.streams) run.queue.set(batchKey(stream), stream);
        if (run.running) return;
        setFeed((previous) => ({ ...previous, syncing: true }));
        const failed = (batch: readonly HistoryStream[]) => {
            for (const { network } of batch) run.failed.add(network);
        };
        const step = async (queued: HistoryStream[]) => {
            const { walletId: wallet, streams: current, noHistory: unindexed } = feedTarget.current;
            // 切换钱包或网络后,排队的批次可能已经不属于当前钱包.
            const known = new Set(current.map(batchKey));
            const batch = queued.filter((stream) => known.has(batchKey(stream)));
            if (!wallet || batch.length === 0) return;
            const result = await syncFeed(wallet, batch, assetsOf, unindexed);
            for (const { network } of batch) {
                run.failed.delete(network);
                run.failures.delete(network);
            }
            for (const network of result.failed) run.failed.add(network);
            for (const { network, code } of result.failures) run.failures.set(network, code);
            const notice = missedNotice(result.missed, ownRef.current, pricesRef.current);
            if (notice) publish(notice);
        };
        await drainQueue(run, SYNC_GATHER, step, failed);
        setFeed({
            syncing: false,
            updatedAt: Date.now(),
            failed: [...run.failed],
            failures: [...run.failures].map(([network, code]) => ({ network, code })),
        });
    }, []);
    const discovery = useQuery({
        queryKey: ["discovery", walletId, supported.join(",")],
        queryFn: async () => {
            for (const account of accounts) {
                if (!chainOf(account.chain).hdAddresses) continue;
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
    useEffect(() => {
        ownRef.current = new Set(addresses.map((row) => addressKey(row.address)));
    }, [addresses]);
    useEffect(() => {
        pricesRef.current = prices.data ?? null;
    }, [prices.data]);
    /** 每批地址收到的最后一个高度,用来发现漏掉的推送. */
    const heights = useRef(new Map<string, number>());
    /** 每批地址上次因为未定论记录触发增量的时间. */
    const settled = useRef(new Map<string, number>());
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
                if (status === "connecting") heights.current.clear();
                // 断线不一定是网关挂了,立刻用 HTTP 确认一次,不等下一轮轮询.
                if (status === "retrying") void client.invalidateQueries({ queryKey: ["version"] });
            },
            onReady: () => {
                void client.invalidateQueries(chainData);
                void pollTxs();
            },
            // 先订阅再取快照,这样两者之间到账的交易不会丢.
            onBatchReady: (target) => void syncNow([target]),
            onLatency: setLatency,
            onTx: (network, tx) => void applyPush(network, tx),
            onTxWatched: () => void pollTxs(),
            onPrices: (raw) => client.setQueryData(["prices"], parsePrices(raw)),
            onUpdate: (network, addresses, raw) => {
                const update = parseUpdate(raw, addresses);
                const { walletId: wallet } = feedTarget.current;
                if (!update || !wallet) return;
                const stream = { network, addresses: [...addresses] };
                const key = batchKey(stream);
                const last = heights.current.get(key);
                heights.current.set(key, update.height);
                // 接不上说明中间漏了推送,交易没取到时网关也会说.这两种情况都要补,只补这批地址.
                const missed = (last !== undefined && update.previous !== last) || update.items === null;
                void applyChainUpdate(wallet, stream, update).then(async (fresh) => {
                    for (const notice of incomingNotices(fresh, ownRef.current, pricesRef.current)) publish(notice);
                    // 新块是唯一的触发.没有未定论的记录时不请求,所以空闲时没有 HTTP.
                    // 第一次只记时间,推送刚带来的记录不用马上再查一遍.
                    if (!settled.current.has(key)) settled.current.set(key, Date.now());
                    const due = Date.now() - (settled.current.get(key) ?? 0) >= SETTLE_INTERVAL;
                    if (missed || (due && (await hasOpenEntries(wallet, stream)))) {
                        settled.current.set(key, Date.now());
                        void syncNow([stream]);
                    }
                });
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

    // 未完成的交易都订阅 tx 频道,包括另一个模式和另一个钱包的.
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

    // 本地到期每隔几秒检查一次.轮询在断线时 15 秒一次,连接正常时只用来发现替换.
    useEffect(() => {
        const timer = setInterval(() => {
            void sweepExpired();
            if (Date.now() - lastPoll.current >= (live ? LIVE_POLL_INTERVAL : POLL_INTERVAL)) void pollTxs();
        }, TICK_INTERVAL);
        return () => clearInterval(timer);
    }, [live, pollTxs]);

    // 交易状态变了,这个标签页和其他标签页缓存的链上数据都要重新查询.
    useEffect(
        () =>
            onSync(() => {
                void client.invalidateQueries(chainData);
            }),
        [client],
    );

    const reconnect = useCallback(() => {
        socketRef.current?.reconnect();
        void client.invalidateQueries({ queryKey: ["version"] });
    }, [client]);

    const failures = feed.failures.filter((failure) => targets.some((target) => target.network === failure.network));
    const everyNetworkFailed = targets.length > 0 && failures.length === targets.length;
    // 全部网络都查询失败也算不可用,版本接口正常不代表上游正常.
    const httpOk = version.isSuccess && !everyNetworkFailed;

    useEffect(() => {
        feedTarget.current = { walletId, streams, noHistory };
    }, [walletId, streams, noHistory]);

    // 正常时这里什么都不做.查询失败的网络单独重试,间隔逐步拉长.断线时靠它保持更新.
    const retries = useRef(0);
    useEffect(() => {
        const broken = new Set([...feed.failed, ...feed.failures.map((failure) => failure.network)]);
        if (live && broken.size === 0) {
            retries.current = 0;
            return;
        }
        const delay = live ? Math.min(POLL_INTERVAL * 2 ** retries.current, MAX_RETRY_INTERVAL) : POLL_INTERVAL;
        const timer = setTimeout(() => {
            retries.current += 1;
            const list = feedTarget.current.streams.filter((stream) => broken.has(stream.network));
            void syncNow(live ? list : undefined);
        }, delay);
        return () => clearTimeout(timer);
    }, [live, feed, syncNow]);

    const value: GatewayState = {
        supported,
        balances,
        failures,
        loading: version.isPending || (feed.updatedAt === 0 && (stored ?? []).length === 0),
        down: !version.isPending && !httpOk,
        httpOk,
        socket: socketState.status,
        retryAt: socketState.retryAt,
        lastUpdated: feed.updatedAt,
        latency: latency ?? version.data?.latency ?? null,
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
