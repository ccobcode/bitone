// 区块浏览器链接内置在网页端，不从网关获取，见 docs/04-security.md 第 5 节。

const TX_LINKS: Record<string, (hash: string) => string> = {
    bitcoin: (hash) => `https://mempool.space/tx/${hash}`,
    "btc-testnet4": (hash) => `https://mempool.space/testnet4/tx/${hash}`,
    "btc-signet": (hash) => `https://mempool.space/signet/tx/${hash}`,
    "btc-testnet3": (hash) => `https://mempool.space/testnet/tx/${hash}`,
    ethereum: (hash) => `https://etherscan.io/tx/${hash}`,
    sepolia: (hash) => `https://sepolia.etherscan.io/tx/${hash}`,
    hoodi: (hash) => `https://hoodi.etherscan.io/tx/${hash}`,
    bsc: (hash) => `https://bscscan.com/tx/${hash}`,
    "bsc-testnet": (hash) => `https://testnet.bscscan.com/tx/${hash}`,
    polygon: (hash) => `https://polygonscan.com/tx/${hash}`,
    amoy: (hash) => `https://amoy.polygonscan.com/tx/${hash}`,
    tron: (hash) => `https://tronscan.org/#/transaction/${hash}`,
    "tron-nile": (hash) => `https://nile.tronscan.org/#/transaction/${hash}`,
    "tron-shasta": (hash) => `https://shasta.tronscan.org/#/transaction/${hash}`,
    solana: (hash) => `https://solscan.io/tx/${hash}`,
    "solana-devnet": (hash) => `https://solscan.io/tx/${hash}?cluster=devnet`,
    "solana-testnet": (hash) => `https://solscan.io/tx/${hash}?cluster=testnet`,
};

export function explorerLink(network: string, hash: string): string | null {
    return TX_LINKS[network]?.(encodeURIComponent(hash)) ?? null;
}
