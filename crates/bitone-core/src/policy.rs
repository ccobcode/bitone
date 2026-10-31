//! 策略常量，见 docs/04-security.md 第 2.7 节。阈值是初始值，技术方案里校准。

use crate::Network;

/// 找零索引超过已签过的最高索引加这个数时按高风险处理。
pub const CHANGE_GAP: u32 = 20;

/// 手续费上限，单位是最小单位。超过时进入高风险样式。
pub fn fee_ceiling(network: Network) -> u128 {
    use Network::*;
    match network {
        Bitcoin | BtcTestnet4 | BtcSignet | BtcTestnet3 => 100_000,
        Ethereum | Sepolia | Hoodi => 50_000_000_000_000_000,
        Bsc | BscTestnet => 20_000_000_000_000_000,
        Polygon | Amoy => 20_000_000_000_000_000_000,
        Tron | TronNile | TronShasta => 200_000_000,
        Solana | SolanaDevnet | SolanaTestnet => 10_000_000,
    }
}

/// Bitcoin 手续费超过发送额的这个百分比时按高风险处理。
pub const BITCOIN_FEE_PERCENT: u64 = 5;
