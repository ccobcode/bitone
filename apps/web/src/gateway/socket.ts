// 余额,交易和价格的订阅.订阅只存在于当前连接,断线后退避重连,重连后重新订阅并让上层用 HTTP 补一次.
import { MAX_ADDRESSES, chunks } from "./client";
import type { Target } from "./assets";

/** 单连接的地址上限,超出的地址靠 HTTP 刷新.和网关一致. */
const MAX_SUBSCRIBED = 5000;
/** 单连接的交易上限,超出的交易靠 HTTP 轮询. */
const MAX_WATCHED_TXS = 100;
const BASE_DELAY = 1000;
const MAX_DELAY = 30_000;
/** 应用层心跳的间隔,两次之间什么都没收到就重连. */
const HEARTBEAT_INTERVAL = 25_000;

/** `degraded` 是连接正常但有订阅被拒绝,这部分地址收不到推送. */
export type SocketStatus = "connecting" | "live" | "retrying" | "degraded";

export interface SocketEvents {
    onStatus(status: SocketStatus, retryAt: number | null): void;
    /** 收到这批地址的新区块推送,内容还没校验. */
    onUpdate(network: string, addresses: readonly string[], update: unknown): void;
    /** 收到交易的最终性变化,内容还没校验. */
    onTx?(network: string, tx: unknown): void;
    /** 新增的交易订阅收到回复.网关先取好基线再回复,这时要用 HTTP 补一次,基线本身不会推送. */
    onTxWatched?(network: string, hash: string): void;
    /** 这批地址的订阅收到回复.网关先取好基线再回复,这时取快照不会漏掉变化,首次,重连和新增都会触发. */
    onBatchReady?(target: Target): void;
    /** 应用层心跳的往返时间,毫秒. */
    onLatency?(ms: number): void;
    onPrices?(prices: unknown): void;
    /**
     * 全部订阅都收到了回复.首次和重连都会触发.
     */
    onReady(): void;
}

export function retryDelay(attempt: number, random = Math.random()): number {
    const base = Math.min(BASE_DELAY * 2 ** attempt, MAX_DELAY);
    return Math.round(base * (0.8 + random * 0.2));
}

export interface WatchedTx {
    network: string;
    hash: string;
}

/** `stale` 是还没等到回复地址就变了,回复后要退订. */
type Topic =
    | { kind: "addresses"; network: string; addresses: readonly string[]; stale?: boolean }
    | { kind: "tx"; network: string; hash: string }
    | { kind: "prices" };

export function batchKey({ network, addresses }: { network: string; addresses: readonly string[] }): string {
    return `${network}|${addresses[0]}`;
}

function txKey({ network, hash }: WatchedTx): string {
    return `${network}|${hash}`;
}

export class BalanceSocket {
    #url: string;
    #events: SocketEvents;
    #targets: Target[] = [];
    #txs = new Map<string, WatchedTx>();
    #socket: WebSocket | null = null;
    #timer: ReturnType<typeof setTimeout> | null = null;
    #attempt = 0;
    #stopped = true;
    #prices = false;
    #heartbeat: ReturnType<typeof setInterval> | null = null;
    #alive = true;
    #pingId = 0;
    #pingAt = 0;
    #nextId = 1;
    /** 请求 ID 对应的主题,收到响应后转成订阅 ID 对应的主题. */
    #pending = new Map<number, Topic>();
    #subscriptions = new Map<string, Topic>();
    #rejected = false;
    /** 连接建立时一批发出的订阅还没有全部回复. */
    #opening = false;

    constructor(url: string, events: SocketEvents) {
        this.#url = url;
        this.#events = events;
    }

    start(): void {
        this.#stopped = false;
        this.#connect();
    }

    stop(): void {
        this.#stopped = true;
        this.#clearTimer();
        this.#drop();
    }

    /** 地址变化时调用.只换变了的那批订阅,连接和其他批次不动,否则每变一次全部都要重查. */
    setTargets(targets: Target[]): void {
        const before = this.#allowed();
        this.#targets = targets;
        const after = this.#allowed();
        const socket = this.#socket;
        if (this.#stopped || socket?.readyState !== WebSocket.OPEN) return;
        const changed = (key: string) => before.get(key)?.addresses.join() !== after.get(key)?.addresses.join();
        for (const [id, topic] of this.#subscriptions) {
            if (topic.kind === "addresses" && changed(batchKey(topic))) this.#unsubscribe(socket, id);
        }
        for (const topic of this.#pending.values()) {
            if (topic.kind === "addresses" && changed(batchKey(topic))) topic.stale = true;
        }
        for (const [key, target] of after) {
            if (changed(key)) this.#subscribeAddresses(socket, target);
        }
    }

    /** 更新要跟踪的交易.交易随时增减,所以只增减这几个订阅,不换连接. */
    watchTxs(list: readonly WatchedTx[]): void {
        const next = new Map(list.slice(0, MAX_WATCHED_TXS).map((tx) => [txKey(tx), tx]));
        const socket = this.#socket;
        const open = socket?.readyState === WebSocket.OPEN;
        if (open) {
            for (const [id, topic] of this.#subscriptions) {
                if (topic.kind === "tx" && !next.has(txKey(topic))) this.#unsubscribe(socket, id);
            }
            for (const [key, tx] of next) if (!this.#txs.has(key)) this.#subscribeTx(socket, tx);
        }
        this.#txs = next;
    }

    /** 主网才有美元价格,所以由上层决定要不要订阅. */
    watchPrices(): void {
        const socket = this.#socket;
        if (!this.#prices && socket?.readyState === WebSocket.OPEN) this.#subscribePrices(socket);
        this.#prices = true;
    }

    /** 立即重连,并清零退避. */
    reconnect(): void {
        this.#attempt = 0;
        this.#clearTimer();
        this.#drop();
        if (!this.#stopped) this.#connect();
    }

    #clearTimer(): void {
        if (this.#timer !== null) clearTimeout(this.#timer);
        this.#timer = null;
    }

    /**
     * 浏览器看不到 WebSocket 的 ping 帧.电脑休眠或切换网络后连接可能已经断了,却不会触发 close,
     * 所以定时发一条应用层 ping,上一条之后什么都没收到就重连.
     */
    #beat(socket: WebSocket): void {
        if (!this.#alive) {
            this.#drop();
            this.#scheduleRetry();
            return;
        }
        this.#ping(socket);
    }

    #ping(socket: WebSocket): void {
        this.#alive = false;
        this.#pingId = this.#nextId++;
        this.#pingAt = performance.now();
        socket.send(JSON.stringify({ jsonrpc: "2.0", id: this.#pingId, method: "ping" }));
    }

    #stopBeat(): void {
        if (this.#heartbeat !== null) clearInterval(this.#heartbeat);
        this.#heartbeat = null;
    }

    /** 摘掉回调再关闭,主动关闭不触发重连. */
    #drop(): void {
        this.#stopBeat();
        const socket = this.#socket;
        this.#socket = null;
        this.#pending.clear();
        this.#subscriptions.clear();
        this.#opening = false;
        if (!socket) return;
        socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
        socket.close();
    }

    #connect(): void {
        this.#events.onStatus("connecting", null);
        const socket = new WebSocket(this.#url);
        this.#socket = socket;
        socket.onopen = () => {
            this.#attempt = 0;
            this.#rejected = false;
            this.#events.onStatus("live", null);
            this.#subscribe(socket);
            this.#opening = this.#pending.size > 0;
            if (!this.#opening) this.#events.onReady();
            this.#ping(socket);
            this.#heartbeat = setInterval(() => this.#beat(socket), HEARTBEAT_INTERVAL);
        };
        socket.onmessage = (event) => {
            this.#alive = true;
            this.#message(event.data);
        };
        socket.onclose = () => this.#scheduleRetry();
    }

    #scheduleRetry(): void {
        this.#stopBeat();
        this.#socket = null;
        this.#pending.clear();
        this.#subscriptions.clear();
        this.#opening = false;
        if (this.#stopped) return;
        const delay = retryDelay(this.#attempt);
        this.#attempt += 1;
        this.#events.onStatus("retrying", Date.now() + delay);
        this.#timer = setTimeout(() => this.#connect(), delay);
    }

    /** 单连接上限内要订阅的批次,键是 `网络|第一个地址`.地址少的批次在前,地址多的网络不能把其他网络挤掉. */
    #allowed(): Map<string, Target> {
        let room = MAX_SUBSCRIBED;
        const allowed = new Map<string, Target>();
        const smallest = [...this.#targets].sort((a, b) => a.addresses.length - b.addresses.length);
        for (const { network, addresses } of smallest) {
            const target = { network, addresses: addresses.slice(0, room) };
            if (target.addresses.length > 0) allowed.set(batchKey(target), target);
            room -= target.addresses.length;
        }
        return allowed;
    }

    #subscribe(socket: WebSocket): void {
        for (const target of this.#allowed().values()) this.#subscribeAddresses(socket, target);
        for (const tx of this.#txs.values()) this.#subscribeTx(socket, tx);
        if (this.#prices) this.#subscribePrices(socket);
    }

    #subscribeAddresses(socket: WebSocket, { network, addresses }: Target): void {
        for (const chunk of chunks(addresses, MAX_ADDRESSES)) {
            this.#request(
                socket,
                { kind: "addresses", network, addresses: chunk },
                { channel: "addresses", network, addresses: chunk },
            );
        }
    }

    #unsubscribe(socket: WebSocket, id: string): void {
        this.#subscriptions.delete(id);
        socket.send(JSON.stringify({ jsonrpc: "2.0", id: this.#nextId++, method: "unsubscribe", params: [id] }));
    }

    #subscribeTx(socket: WebSocket, { network, hash }: WatchedTx): void {
        this.#request(socket, { kind: "tx", network, hash }, { channel: "tx", network, hash });
    }

    #subscribePrices(socket: WebSocket): void {
        this.#request(socket, { kind: "prices" }, { channel: "prices" });
    }

    #request(socket: WebSocket, topic: Topic, params: unknown): void {
        const id = this.#nextId++;
        this.#pending.set(id, topic);
        socket.send(JSON.stringify({ jsonrpc: "2.0", id, method: "subscribe", params }));
    }

    #message(data: unknown): void {
        if (typeof data !== "string") return;
        let message: { id?: unknown; result?: unknown; method?: unknown; params?: Record<string, unknown> };
        try {
            message = JSON.parse(data);
        } catch {
            return;
        }
        if (typeof message.id === "number") {
            if (message.id === this.#pingId) {
                this.#events.onLatency?.(Math.round(performance.now() - this.#pingAt));
                return;
            }
            const topic = this.#pending.get(message.id);
            if (!topic) return;
            this.#pending.delete(message.id);
            const stale = topic.kind === "addresses" && topic.stale;
            const id = typeof message.result === "string" ? message.result : null;
            if (stale) {
                if (id && this.#socket) this.#unsubscribe(this.#socket, id);
            } else if (id) this.#subscriptions.set(id, topic);
            else this.#rejected = true;
            if (!this.#opening && topic.kind === "tx") this.#events.onTxWatched?.(topic.network, topic.hash);
            if (topic.kind === "addresses" && !stale && id) {
                this.#events.onBatchReady?.({ network: topic.network, addresses: [...topic.addresses] });
            }
            if (this.#pending.size > 0) return;
            if (this.#rejected) this.#events.onStatus("degraded", null);
            if (this.#opening) {
                this.#opening = false;
                this.#events.onReady();
            }
            return;
        }
        if (message.method !== "update") return;
        const { subscription, tx, prices } = message.params ?? {};
        const topic = typeof subscription === "string" ? this.#subscriptions.get(subscription) : undefined;
        if (topic?.kind === "addresses") this.#events.onUpdate(topic.network, topic.addresses, message.params);
        if (topic?.kind === "tx") this.#events.onTx?.(topic.network, tx);
        if (topic?.kind === "prices") this.#events.onPrices?.(prices);
    }
}
