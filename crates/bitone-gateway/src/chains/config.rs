//! 一个网络的配置.各条链自己的选项由 `ChainSpec` 检查.

use super::{is_valid, spec};
use bitone_core::{Mode, Network};
use serde::Deserialize;
use std::net::IpAddr;

// ponytail: 各条链的选项都在这一个结构里,配置文件的格式才能不变.`deny_unknown_fields` 和 `flatten`
// 不能同用,所以新链要加选项时还是改这里.选项多了以后按链分表.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub upstreams: Vec<String>,
    #[serde(default)]
    pub tokens: Vec<String>,
    /// TronGrid 的 API key,发给这个网络的全部上游.
    pub tron_api_key: Option<String>,
    /// EVM 的 Etherscan 兼容索引服务,只用于交易历史.节点查不到地址的历史.
    pub indexer: Option<String>,
    /// HyperSync 的 token.配了它,`indexer` 就按 HyperSync 的接口访问,否则按 Etherscan 的接口.
    pub indexer_token: Option<String>,
}

pub fn check_url(url: &str, owner: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("{owner} 的上游地址无效"))?;
    if matches!(parsed.scheme(), "http" | "https") {
        Ok(())
    } else {
        Err(format!("{owner} 的上游只支持 http 和 https"))
    }
}

/// 同机和内网的地址不经过公网.主机名查不出在哪里,不算.
fn is_local(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.trim_matches(['[', ']']).parse() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unique_local(),
        Err(_) => false,
    }
}

/// 主网的上游决定网页端看到的余额和交易状态,明文连接谁都能改.同机或内网自建的节点除外.
fn check_transport(network: Network, url: &str) -> Result<(), String> {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Ok(());
    };
    let plain = matches!(parsed.scheme(), "http" | "tcp");
    if network.mode() == Mode::Mainnet && plain && !parsed.host_str().is_some_and(is_local) {
        return Err(format!("{} 是主网,明文上游只能是本机或内网的 IP", network.id()));
    }
    Ok(())
}

impl NetworkConfig {
    pub fn validate(&self, network: Network) -> Result<(), String> {
        let id = network.id();
        let spec = spec(network.chain());
        let written = [
            ("tokens", !self.tokens.is_empty()),
            ("tron_api_key", self.tron_api_key.is_some()),
            ("indexer", self.indexer.is_some()),
            ("indexer_token", self.indexer_token.is_some()),
        ];
        for (option, set) in written {
            if set && !spec.options().contains(&option) {
                return Err(format!("{id} 不支持 {option}"));
            }
        }
        for url in self.upstreams.iter().chain(&self.indexer) {
            check_transport(network, url)?;
        }
        for url in &self.upstreams {
            spec.check_upstream(network, url)?;
        }
        if let Some(token) = self.tokens.iter().find(|t| !is_valid(network, t)) {
            return Err(format!("{id} 的代币地址无效: {token}"));
        }
        spec.check(network, self)
    }
}
