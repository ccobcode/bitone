//! 网络定义，见 docs/03-system.md 第 5 节。

use minicbor::{Decode, Encode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Encode, Decode)]
#[cbor(index_only)]
pub enum Chain {
    #[n(0)]
    Bitcoin,
    #[n(1)]
    Evm,
    #[n(2)]
    Tron,
    #[n(3)]
    Solana,
}

impl Chain {
    pub fn id(self) -> &'static str {
        match self {
            Self::Bitcoin => "bitcoin",
            Self::Evm => "evm",
            Self::Tron => "tron",
            Self::Solana => "solana",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Bitcoin, Self::Evm, Self::Tron, Self::Solana]
            .into_iter()
            .find(|chain| chain.id() == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Mainnet,
    Testnet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Network {
    Bitcoin,
    BtcTestnet4,
    BtcSignet,
    BtcTestnet3,
    Ethereum,
    Sepolia,
    Hoodi,
    Bsc,
    BscTestnet,
    Polygon,
    Amoy,
    Tron,
    TronNile,
    TronShasta,
    Solana,
    SolanaDevnet,
    SolanaTestnet,
}

impl Network {
    pub const ALL: [Network; 17] = [
        Network::Bitcoin,
        Network::BtcTestnet4,
        Network::BtcSignet,
        Network::BtcTestnet3,
        Network::Ethereum,
        Network::Sepolia,
        Network::Hoodi,
        Network::Bsc,
        Network::BscTestnet,
        Network::Polygon,
        Network::Amoy,
        Network::Tron,
        Network::TronNile,
        Network::TronShasta,
        Network::Solana,
        Network::SolanaDevnet,
        Network::SolanaTestnet,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Network::Bitcoin => "bitcoin",
            Network::BtcTestnet4 => "btc-testnet4",
            Network::BtcSignet => "btc-signet",
            Network::BtcTestnet3 => "btc-testnet3",
            Network::Ethereum => "ethereum",
            Network::Sepolia => "sepolia",
            Network::Hoodi => "hoodi",
            Network::Bsc => "bsc",
            Network::BscTestnet => "bsc-testnet",
            Network::Polygon => "polygon",
            Network::Amoy => "amoy",
            Network::Tron => "tron",
            Network::TronNile => "tron-nile",
            Network::TronShasta => "tron-shasta",
            Network::Solana => "solana",
            Network::SolanaDevnet => "solana-devnet",
            Network::SolanaTestnet => "solana-testnet",
        }
    }

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Network::Bitcoin => "Bitcoin",
            Network::BtcTestnet4 => "Testnet4",
            Network::BtcSignet => "Signet",
            Network::BtcTestnet3 => "Testnet3",
            Network::Ethereum => "Ethereum",
            Network::Sepolia => "Sepolia",
            Network::Hoodi => "Hoodi",
            Network::Bsc => "BNB Smart Chain",
            Network::BscTestnet => "BSC Testnet",
            Network::Polygon => "Polygon",
            Network::Amoy => "Amoy",
            Network::Tron => "TRON",
            Network::TronNile => "Nile",
            Network::TronShasta => "Shasta",
            Network::Solana => "Solana",
            Network::SolanaDevnet => "Devnet",
            Network::SolanaTestnet => "Solana Testnet",
        }
    }

    pub fn from_id(id: &str) -> Option<Network> {
        Network::ALL.into_iter().find(|n| n.id() == id)
    }

    pub fn chain(self) -> Chain {
        match self {
            Network::Bitcoin | Network::BtcTestnet4 | Network::BtcSignet | Network::BtcTestnet3 => Chain::Bitcoin,
            Network::Tron | Network::TronNile | Network::TronShasta => Chain::Tron,
            Network::Solana | Network::SolanaDevnet | Network::SolanaTestnet => Chain::Solana,
            _ => Chain::Evm,
        }
    }

    pub fn mode(self) -> Mode {
        match self {
            Network::Bitcoin
            | Network::Ethereum
            | Network::Bsc
            | Network::Polygon
            | Network::Tron
            | Network::Solana => Mode::Mainnet,
            _ => Mode::Testnet,
        }
    }

    pub fn evm_chain_id(self) -> Option<u64> {
        match self {
            Network::Ethereum => Some(1),
            Network::Sepolia => Some(11155111),
            Network::Hoodi => Some(560048),
            Network::Bsc => Some(56),
            Network::BscTestnet => Some(97),
            Network::Polygon => Some(137),
            Network::Amoy => Some(80002),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for n in Network::ALL {
            assert_eq!(Network::from_id(n.id()), Some(n));
        }
        assert_eq!(Network::from_id("dogecoin"), None);
    }

    #[test]
    fn every_chain_has_a_testnet() {
        for chain in [Chain::Bitcoin, Chain::Evm, Chain::Tron, Chain::Solana] {
            assert!(
                Network::ALL
                    .iter()
                    .any(|n| n.chain() == chain && n.mode() == Mode::Testnet)
            );
        }
    }

    #[test]
    fn only_evm_has_chain_id() {
        for n in Network::ALL {
            assert_eq!(n.evm_chain_id().is_some(), n.chain() == Chain::Evm);
        }
    }
}
