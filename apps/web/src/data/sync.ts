// 多标签页同步。IndexedDB 的变更由 liveQuery 带到各页面，这里通知其他标签页刷新网关缓存，
// 比如另一个标签页发出交易后，这里的 nonce、UTXO、余额和历史都要重新查询。
type Listener = () => void;

const listeners = new Set<Listener>();
const channel = typeof BroadcastChannel === "function" ? new BroadcastChannel("bitone-sync") : null;

function notify(): void {
    for (const listener of listeners) listener();
}

if (channel) {
    channel.onmessage = notify;
    // Node 里打开的频道会拖住进程退出，浏览器没有这个方法。
    const { unref } = channel as unknown as { unref?(): void };
    unref?.call(channel);
}

export function announce(): void {
    notify();
    channel?.postMessage("txs");
}

export function onSync(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}
