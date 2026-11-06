//! 配置文件.示例见 gateway.example.toml.

use crate::chains::{NetworkConfig, check_url, is_valid};
use axum::http::{HeaderName, HeaderValue};
use bitone_core::{Mode, Network};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

const COINGECKO_SIMPLE_PRICE: &str = "https://api.coingecko.com/api/v3/simple/price";

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    /// 配置后同进程托管网页端静态文件,即自部署模式.
    pub web_dir: Option<PathBuf>,
    /// 一个上游一次查询的总时限,超时切换下一个.
    pub upstream_timeout_ms: u64,
    /// 交易,高度,价格这几个单值频道轮询上游的间隔.
    pub ws_poll_interval_ms: u64,
    /// 上游没有长连接时轮询区块高度的间隔.
    pub follow_poll_interval_ms: u64,
    pub ws_heartbeat_ms: u64,
    pub rate_limit_burst: f64,
    pub rate_limit_per_second: f64,
    /// 只有来自这些 IP 的请求才读 X-Forwarded-For.默认为空,不信任任何代理.
    pub trusted_proxies: Vec<IpAddr>,
    pub ws_max_connections_per_ip: usize,
    /// 一个 IP 全部连接合计能订阅的地址数.
    pub ws_max_addresses_per_ip: usize,
    pub prices: Option<PricesConfig>,
    #[serde(deserialize_with = "networks")]
    pub networks: HashMap<Network, NetworkConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricesConfig {
    /// CoinGecko 的 simple/price 格式.
    #[serde(default = "coingecko_simple_price")]
    pub url: String,
    pub api_key_header: Option<String>,
    pub api_key: Option<String>,
    /// `网络:合约` 到价格源资产 ID 的映射.原生币的合约写 `native`.
    pub ids: BTreeMap<String, String>,
}

fn coingecko_simple_price() -> String {
    COINGECKO_SIMPLE_PRICE.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8787)),
            web_dir: None,
            upstream_timeout_ms: 10_000,
            // Electrum 查交易状态每次都要开一条 TLS 连接,太频繁公共服务器会限流.
            ws_poll_interval_ms: 15_000,
            follow_poll_interval_ms: 6_000,
            ws_heartbeat_ms: 30_000,
            // 放宽阈值,运营商 NAT 下很多用户共用一个 IP.
            rate_limit_burst: 1200.0,
            rate_limit_per_second: 20.0,
            trusted_proxies: Vec::new(),
            ws_max_connections_per_ip: 64,
            ws_max_addresses_per_ip: 20_000,
            prices: None,
            networks: HashMap::new(),
        }
    }
}

fn networks<'de, D: Deserializer<'de>>(deserializer: D) -> Result<HashMap<Network, NetworkConfig>, D::Error> {
    HashMap::<String, NetworkConfig>::deserialize(deserializer)?
        .into_iter()
        .map(|(id, network)| match Network::from_id(&id) {
            Some(known) => Ok((known, network)),
            None => Err(D::Error::custom(format!("未知网络 {id}"))),
        })
        .collect()
}

impl PricesConfig {
    fn validate(&self) -> Result<(), String> {
        check_url(&self.url, "prices")?;
        match (&self.api_key_header, &self.api_key) {
            (None, None) => {}
            (Some(header), Some(key)) => {
                if HeaderName::try_from(header.as_str()).is_err() || HeaderValue::from_str(key).is_err() {
                    return Err("prices 的 API key 请求头无效".into());
                }
            }
            _ => return Err("prices 的 api_key_header 和 api_key 要同时配置".into()),
        }
        for key in self.ids.keys() {
            let (network, asset) = key
                .split_once(':')
                .ok_or_else(|| format!("价格键 {key} 应为 网络:合约"))?;
            let network = Network::from_id(network).ok_or_else(|| format!("价格键 {key} 的网络未知"))?;
            if network.mode() != Mode::Mainnet {
                return Err(format!("价格键 {key} 不是主网资产"));
            }
            if asset != "native" && !is_valid(network, asset) {
                return Err(format!("价格键 {key} 的合约地址无效"));
            }
        }
        Ok(())
    }
}

/// 命令行参数,优先于配置文件.镜像靠它固定监听地址和网页目录,配置文件不用改就能挂载.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub config: Option<PathBuf>,
    pub listen: Option<SocketAddr>,
    pub web_dir: Option<PathBuf>,
    pub trusted_proxies: Vec<IpAddr>,
}

impl Options {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
        let mut options = Options::default();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} 缺少参数"));
            match arg.as_str() {
                "--config" | "-c" => options.config = Some(value()?.into()),
                "--listen" => options.listen = Some(value()?.parse().map_err(|e| format!("--listen 无效: {e}"))?),
                "--web-dir" => options.web_dir = Some(value()?.into()),
                "--trusted-proxy" => options
                    .trusted_proxies
                    .push(value()?.parse().map_err(|e| format!("--trusted-proxy 无效: {e}"))?),
                _ if arg.starts_with('-') || options.config.is_some() => return Err(format!("未知参数 {arg}")),
                _ => options.config = Some(arg.into()),
            }
        }
        Ok(options)
    }
}

impl Config {
    pub fn load(options: Options) -> Result<Config, String> {
        let path = options.config.as_deref().unwrap_or(Path::new("gateway.toml"));
        let text = std::fs::read_to_string(path).map_err(|e| format!("读取配置 {} 失败: {e}", path.display()))?;
        let mut config = Config::read(&text)?;
        config.listen = options.listen.unwrap_or(config.listen);
        config.web_dir = options.web_dir.or(config.web_dir);
        config.trusted_proxies.extend(options.trusted_proxies);
        config.validate()?;
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let config = Config::read(text)?;
        config.validate()?;
        Ok(config)
    }

    fn read(text: &str) -> Result<Config, String> {
        let mut config: Config = toml::from_str(text).map_err(|e| format!("配置格式错误: {e}"))?;
        // 没有上游等于没有配置这个网络.
        config.networks.retain(|_, network| !network.upstreams.is_empty());
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        let periods = [
            self.upstream_timeout_ms,
            self.ws_poll_interval_ms,
            self.follow_poll_interval_ms,
            self.ws_heartbeat_ms,
        ];
        if periods.contains(&0) {
            return Err("超时和间隔必须大于 0".into());
        }
        if self.rate_limit_burst < 1.0 || self.rate_limit_per_second <= 0.0 {
            return Err("rate_limit_burst 至少为 1,rate_limit_per_second 必须大于 0".into());
        }
        if self.ws_max_connections_per_ip == 0 {
            return Err("ws_max_connections_per_ip 至少为 1".into());
        }
        if self.ws_max_addresses_per_ip == 0 {
            return Err("ws_max_addresses_per_ip 至少为 1".into());
        }
        if let Some(dir) = &self.web_dir
            && !dir.join("index.html").is_file()
        {
            return Err(format!("web_dir {} 里没有 index.html", dir.display()));
        }
        if let Some(prices) = &self.prices {
            prices.validate()?;
        }
        self.networks
            .iter()
            .try_for_each(|(network, config)| config.validate(*network))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitone_core::manifest::Manifest;

    const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";

    fn options(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(ToString::to_string))
    }

    #[test]
    fn command_line_options_are_parsed() {
        let parsed = options(&[
            "--config",
            "a.toml",
            "--listen",
            "0.0.0.0:8787",
            "--web-dir",
            "web",
            "--trusted-proxy",
            "172.30.0.2",
            "--trusted-proxy",
            "::1",
        ])
        .unwrap();
        assert_eq!(parsed.config, Some("a.toml".into()));
        assert_eq!(parsed.listen, Some("0.0.0.0:8787".parse().unwrap()));
        assert_eq!(parsed.web_dir, Some("web".into()));
        assert_eq!(parsed.trusted_proxies.len(), 2);
        assert_eq!(options(&["a.toml"]).unwrap().config, Some("a.toml".into()));
        assert_eq!(options(&[]).unwrap(), Options::default());
    }

    #[test]
    fn bad_command_lines_are_rejected() {
        for args in [
            &["--listen"][..],
            &["--listen", "nope"],
            &["--trusted-proxy", "10.0.0.0/8"],
            &["--typo", "x"],
            &["a.toml", "b.toml"],
        ] {
            assert!(options(args).is_err(), "{args:?}");
        }
    }

    /// 命令行的目录替换配置文件里的,所以配置文件里的旧目录不存在时也能启动.
    #[test]
    fn command_line_overrides_the_file() {
        let dir = std::env::temp_dir().join(format!("bitone-config-{}", std::process::id()));
        let web = dir.join("web");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("index.html"), "").unwrap();
        let file = dir.join("gateway.toml");
        std::fs::write(
            &file,
            "listen = '127.0.0.1:1'\nweb_dir = '/nonexistent/bitone'\ntrusted_proxies = ['10.0.0.1']",
        )
        .unwrap();

        assert!(
            Config::load(Options {
                config: Some(file.clone()),
                ..Options::default()
            })
            .is_err()
        );
        let config = Config::load(Options {
            config: Some(file),
            listen: Some("0.0.0.0:8787".parse().unwrap()),
            web_dir: Some(web.clone()),
            trusted_proxies: vec!["172.30.0.2".parse().unwrap()],
        })
        .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(config.listen.to_string(), "0.0.0.0:8787");
        assert_eq!(config.web_dir, Some(web));
        assert_eq!(config.trusted_proxies.len(), 2, "命令行的代理加在配置文件的后面");
    }

    #[test]
    fn example_config_is_valid() {
        let config = Config::parse(include_str!("../gateway.example.toml")).unwrap();
        assert_eq!(config.listen.to_string(), "127.0.0.1:8787");
        assert!(config.trusted_proxies.is_empty(), "默认不信任任何代理");
    }

    /// 配置漏掉的网络和代币,网页端完全看不到,所以示例配置必须覆盖内置清单.
    #[test]
    fn example_config_covers_the_builtin_manifest() {
        let config = Config::parse(include_str!("../gateway.example.toml")).unwrap();
        for network in Network::ALL {
            assert!(config.networks.contains_key(&network), "{}", network.id());
        }
        let ids = config.prices.unwrap().ids;
        for asset in &Manifest::builtin().assets {
            let network = Network::from_id(&asset.network).unwrap();
            if asset.contract != "native" {
                assert!(
                    config.networks[&network].tokens.contains(&asset.contract),
                    "{}",
                    asset.key()
                );
            }
            if network.mode() == Mode::Mainnet {
                assert!(ids.contains_key(&asset.key()), "{}", asset.key());
            }
        }
    }

    #[test]
    fn empty_upstreams_mean_not_configured() {
        let config = Config::parse("[networks.bitcoin]\nupstreams = []").unwrap();
        assert!(config.networks.is_empty());
    }

    #[test]
    fn prices_url_defaults_to_coingecko() {
        let config = Config::parse("[prices.ids]\n'bitcoin:native' = 'bitcoin'").unwrap();
        assert_eq!(config.prices.unwrap().url, COINGECKO_SIMPLE_PRICE);
    }

    #[test]
    fn accepts_the_optional_keys() {
        let text = format!(
            "trusted_proxies = ['10.0.0.1', '2001:db8::1']\n\
             [prices]\napi_key_header = 'x-cg-demo-api-key'\napi_key = 'k'\n\
             [prices.ids]\n'ethereum:{USDT}' = 'tether'\n\
             [networks.tron]\nupstreams = ['https://a']\ntron_api_key = 'k'\n\
             [networks.ethereum]\nupstreams = ['https://a']\nindexer = 'https://eth.blockscout.com/api'\n"
        );
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.trusted_proxies.len(), 2);
        assert_eq!(config.networks[&Network::Tron].tron_api_key.as_deref(), Some("k"));
    }

    fn network(id: &str, option: &str, url: &str) -> Result<Config, String> {
        Config::parse(&format!(
            "[networks.{id}]\n{option} = '{url}'\nupstreams = ['https://a']"
        ))
    }

    fn upstream(id: &str, url: &str) -> Result<Config, String> {
        Config::parse(&format!("[networks.{id}]\nupstreams = ['{url}']"))
    }

    #[test]
    fn mainnet_refuses_plaintext_to_public_servers() {
        for url in [
            "tcp://electrum.example:50001",
            "tcp://8.8.8.8:50001",
            "http://node.example",
        ] {
            assert!(upstream("bitcoin", url).is_err(), "{url}");
            assert!(upstream("btc-testnet4", url).is_ok(), "{url}");
        }
        assert!(upstream("ethereum", "http://node.example").is_err());
        assert!(upstream("tron", "http://8.8.8.8:8090").is_err());
        assert!(upstream("solana", "http://node.example").is_err());
        assert!(network("ethereum", "indexer", "http://indexer.example").is_err());
        assert!(network("sepolia", "indexer", "http://indexer.example").is_ok());
    }

    /// 自建节点常常不加密,流量不出这台机器或内网.
    #[test]
    fn mainnet_accepts_plaintext_to_local_servers() {
        for url in [
            "tcp://127.0.0.1:50001",
            "tcp://localhost:50001",
            "tcp://10.0.0.5:50001",
            "http://192.168.1.2:3000",
            "http://172.20.0.3:3000",
            "http://[::1]:3000",
            "http://[fd00::1]:3000",
        ] {
            assert!(upstream("bitcoin", url).is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_bad_config() {
        for text in [
            "[networks.dogecoin]\nupstreams = ['https://a']",
            "[networks.bitcoin]\nupstreams = ['ftp://a']",
            "[networks.bitcoin]\nupstreams = ['https://a']\ntokens = ['x']",
            "[networks.ethereum]\nupstreams = ['https://a']\ntokens = ['0x12']",
            "[networks.ethereum]\nupstreams = ['https://a']\ntron_api_key = 'k'",
            "[networks.tron]\nupstreams = ['https://a']\ntron_api_key = \"a\nb\"",
            "[networks.tron]\nupstreams = ['https://a']\nindexer = 'https://b'",
            "[networks.ethereum]\nupstreams = ['https://a']\nindexer = 'ftp://b'",
            "[networks.ethereum]\nupstreams = ['https://a']\nindexer_token = 'k'",
            "[networks.ethereum]\nupstreams = ['https://a']\nindexer = 'https://b'\nindexer_token = \"a\nb\"",
            "web_dir = '/nonexistent/bitone'",
            "ws_heartbeat_ms = 0",
            "follow_poll_interval_ms = 0",
            "ws_max_connections_per_ip = 0",
            "ws_max_addresses_per_ip = 0",
            "rate_limit_burst = 0",
            "trusted_proxies = ['10.0.0.0/8']",
            "trusted_proxies = ['not-an-ip']",
            "typo = 1",
            "[prices]",
            "[prices.ids]\n'bitcoin' = 'bitcoin'",
            "[prices.ids]\n'dogecoin:native' = 'dogecoin'",
            "[prices.ids]\n'sepolia:native' = 'ethereum'",
            "[prices.ids]\n'ethereum:0x12' = 'x'",
            "[prices]\nurl = 'ftp://a'\n[prices.ids]\n'bitcoin:native' = 'bitcoin'",
            "[prices]\napi_key = 'k'\n[prices.ids]\n'bitcoin:native' = 'bitcoin'",
            "[prices]\napi_key_header = 'bad header'\napi_key = 'k'\n[prices.ids]\n'bitcoin:native' = 'bitcoin'",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
    }
}
