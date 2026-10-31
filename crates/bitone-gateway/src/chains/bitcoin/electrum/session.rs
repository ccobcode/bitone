//! Electrum 的连接：上游地址的写法、TLS 和请求响应。

use super::{history_row, not_found, output_of, script_hash};
use crate::upstream::Failed;
use bitcoin::consensus::deserialize;
use bitcoin::hex::FromHex;
use bitcoin::{Transaction, TxOut, Txid};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// 一次写入的请求数。公共服务器按连接计费，太大容易被断开。
const BATCH: usize = 100;
/// 单行响应的上限。上游的数据不可信，不能让它撑爆内存。
const MAX_LINE: u64 = 16 << 20;

/// 上游地址的写法，决定是否验证证书。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// `tcp://`，只用于同机或内网自建的服务器。
    Plain,
    /// `ssl://`，验证证书。
    Tls,
    /// `ssl-insecure://`，不验证证书。公共测试网服务器大多是自签名，配置层拒绝主网这样写。
    TlsUnverified,
}

pub fn transport(url: &str) -> Option<Transport> {
    match url.split_once("://")?.0 {
        "tcp" => Some(Transport::Plain),
        "ssl" => Some(Transport::Tls),
        "ssl-insecure" => Some(Transport::TlsUnverified),
        _ => None,
    }
}

/// 不信任证书，也就不用核对握手签名。公共测试网的自签名证书是 X.509 v1，核对时解析不了。
#[derive(Debug)]
struct AcceptAnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub(super) fn tls_config(transport: Transport) -> Option<Arc<ClientConfig>> {
    if transport == Transport::Plain {
        return None;
    }
    let provider = Arc::new(ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .ok()?;
    let config = if transport == Transport::Tls {
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
            .with_no_client_auth()
    };
    Some(Arc::new(config))
}

pub(super) trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub(super) struct RpcError {
    pub(super) code: i64,
    pub(super) message: String,
}

pub(super) type Reply = Result<Value, RpcError>;
pub(super) type Call = (&'static str, Value);

pub(super) struct Session {
    pub(super) stream: BufReader<Box<dyn Stream>>,
    pub(super) next: u64,
    /// 读到一半的行。等推送时读取可能被打断，下次接着读。
    pub(super) line: Vec<u8>,
    /// 等回复时收到的推送，回复处理完再看。
    pub(super) pushes: Vec<Value>,
}

impl Session {
    /// 读一条消息。可以被 `select!` 打断，读到一半的内容留在 `line` 里。
    pub(super) async fn read_message(&mut self) -> Result<Value, Failed> {
        let read = (&mut self.stream)
            .take(MAX_LINE)
            .read_until(b'\n', &mut self.line)
            .await;
        if read.map_err(|_| Failed)? == 0 || self.line.len() as u64 >= MAX_LINE {
            return Err(Failed);
        }
        let message = serde_json::from_slice(&self.line).map_err(|_| Failed);
        self.line.clear();
        message
    }

    pub(super) async fn read_reply(&mut self) -> Result<(u64, Reply), Failed> {
        loop {
            let message = self.read_message().await?;
            // 区块头和地址订阅的推送没有 id。
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                self.pushes.push(message);
                continue;
            };
            let error = message.get("error").filter(|error| !error.is_null());
            let reply = match error {
                None => Ok(message.get("result").cloned().ok_or(Failed)?),
                Some(error) => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                }),
            };
            return Ok((id, reply));
        }
    }

    /// 批量请求，结果顺序和请求相同。连接级的失败是 `Failed`，单项的错误留给调用方判断。
    pub(super) async fn call(&mut self, calls: &[Call]) -> Result<Vec<Reply>, Failed> {
        let mut replies = Vec::with_capacity(calls.len());
        for chunk in calls.chunks(BATCH) {
            let first = self.next;
            let mut payload = String::new();
            for (offset, (method, params)) in chunk.iter().enumerate() {
                let id = first + offset as u64;
                payload.push_str(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
                payload.push('\n');
            }
            self.next += chunk.len() as u64;
            let writer = self.stream.get_mut();
            writer.write_all(payload.as_bytes()).await.map_err(|_| Failed)?;
            writer.flush().await.map_err(|_| Failed)?;
            let mut slots: Vec<Option<Reply>> = chunk.iter().map(|_| None).collect();
            for _ in 0..chunk.len() {
                let (id, reply) = self.read_reply().await?;
                let index = id.checked_sub(first).ok_or(Failed)? as usize;
                *slots.get_mut(index).ok_or(Failed)? = Some(reply);
            }
            for slot in slots {
                replies.push(slot.ok_or(Failed)?);
            }
        }
        Ok(replies)
    }

    pub(super) async fn one(&mut self, method: &'static str, params: Value) -> Result<Value, Failed> {
        self.call(&[(method, params)])
            .await?
            .pop()
            .ok_or(Failed)?
            .map_err(|_| Failed)
    }

    pub(super) async fn tip(&mut self) -> Result<u64, Failed> {
        let header = self.one("blockchain.headers.subscribe", json!([])).await?;
        header.get("height").and_then(Value::as_u64).ok_or(Failed)
    }

    /// 取一批原始交易。查不到的不在结果里。
    pub(super) async fn transactions(&mut self, txids: &BTreeSet<Txid>) -> Result<HashMap<Txid, Transaction>, Failed> {
        let calls: Vec<Call> = txids
            .iter()
            .map(|txid| ("blockchain.transaction.get", json!([txid.to_string(), false])))
            .collect();
        let mut found = HashMap::new();
        for (txid, reply) in txids.iter().zip(self.call(&calls).await?) {
            match reply {
                Ok(hex) => {
                    found.insert(*txid, decode(hex.as_str().ok_or(Failed)?)?);
                }
                Err(error) if not_found(&error) => {}
                Err(_) => return Err(Failed),
            }
        }
        Ok(found)
    }

    /// 交易所在的高度，`None` 是还在内存池。协议没有按交易查高度的方法，Blockstream 和
    /// mempool.space 的服务器也不支持 verbose，所以从交易涉及的脚本历史里找。
    /// 历史太长的脚本服务器会报错，OP_RETURN 不进索引，所以输入和输出都查，找到一个就够。
    /// 都找不到时调用方换上游。
    pub(super) async fn heights(
        &mut self,
        txs: &HashMap<Txid, Transaction>,
        prevs: &HashMap<Txid, Transaction>,
    ) -> Result<HashMap<Txid, Option<u64>>, Failed> {
        let outputs: Vec<(Txid, &TxOut)> = txs
            .iter()
            .flat_map(|(txid, tx)| {
                let spent = tx
                    .input
                    .iter()
                    .filter_map(|input| output_of(prevs, input.previous_output).ok());
                tx.output.iter().chain(spent).map(|output| (*txid, output))
            })
            .collect();
        let calls: Vec<Call> = outputs
            .iter()
            .map(|(_, output)| {
                let hash = script_hash(&output.script_pubkey);
                ("blockchain.scripthash.get_history", json!([hash]))
            })
            .collect();
        let mut heights = HashMap::new();
        for ((txid, _), reply) in outputs.iter().zip(self.call(&calls).await?) {
            let Ok(rows) = reply else {
                continue;
            };
            for row in rows.as_array().ok_or(Failed)? {
                let (found, height) = history_row(row)?;
                if found == *txid {
                    heights.insert(found, height);
                }
            }
        }
        Ok(heights)
    }

    /// 这批交易花费的前序交易，用来知道输入的地址和金额。
    pub(super) async fn previous(&mut self, txs: &[&Transaction]) -> Result<HashMap<Txid, Transaction>, Failed> {
        let txids: BTreeSet<Txid> = txs
            .iter()
            .flat_map(|tx| &tx.input)
            .map(|input| input.previous_output)
            .filter(|outpoint| !outpoint.is_null())
            .map(|outpoint| outpoint.txid)
            .collect();
        self.transactions(&txids).await
    }

    /// 区块时间在区块头的第 68 字节，小端 4 字节。
    pub(super) async fn block_times(&mut self, heights: &BTreeSet<u64>) -> Result<HashMap<u64, u32>, Failed> {
        let calls: Vec<Call> = heights
            .iter()
            .map(|height| ("blockchain.block.header", json!([height])))
            .collect();
        let mut times = HashMap::new();
        for (height, reply) in heights.iter().zip(self.call(&calls).await?) {
            let reply = reply.map_err(|_| Failed)?;
            let header = Vec::<u8>::from_hex(reply.as_str().ok_or(Failed)?).map_err(|_| Failed)?;
            let bytes: [u8; 4] = header.get(68..72).ok_or(Failed)?.try_into().map_err(|_| Failed)?;
            times.insert(*height, u32::from_le_bytes(bytes));
        }
        Ok(times)
    }
}

pub(super) fn decode(hex: &str) -> Result<Transaction, Failed> {
    deserialize(&Vec::<u8>::from_hex(hex).map_err(|_| Failed)?).map_err(|_| Failed)
}
