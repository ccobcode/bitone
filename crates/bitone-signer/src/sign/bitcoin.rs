//! Bitcoin 签名.输入是否属于本钱包,用哪条路径,签什么摘要,都由 `prepare` 决定,
//! 和核对时走同一份逻辑.私钥派生出来后先核对 script,再签名.

use crate::Error;
use crate::keys::Wallet;
use bitcoin::key::TapTweak;
use bitcoin::secp256k1::{All, Keypair, Message, Secp256k1, SecretKey};
use bitcoin::sighash::{EcdsaSighashType, TapSighashType};
use bitcoin::{ScriptBuf, ecdsa, taproot};
use bitone_core::request::SignRequest;
use bitone_core::tx::bitcoin::{OwnInput, Prepared, Sighash, prepare, script_for};
use bitone_core::{Mode, Network};

/// RFC 6979 的确定性签名,低 s,末尾带 sighash 字节.
fn sign_ecdsa(secp: &Secp256k1<All>, key: &SecretKey, digest: [u8; 32], ty: EcdsaSighashType) -> Vec<u8> {
    let signature = secp.sign_ecdsa(&Message::from_digest(digest), key);
    ecdsa::Signature {
        signature,
        sighash_type: ty,
    }
    .to_vec()
}

/// BIP86 的 key path 签名,不用随机数,同一笔交易的结果不变.DEFAULT 时没有尾字节.
fn sign_taproot(secp: &Secp256k1<All>, key: &SecretKey, digest: [u8; 32], ty: TapSighashType) -> Vec<u8> {
    let mut keypair = Keypair::from_secret_key(secp, key);
    let mut tweaked = keypair.tap_tweak(secp, None).to_keypair();
    let signature = secp.sign_schnorr_no_aux_rand(&Message::from_digest(digest), &tweaked);
    keypair.non_secure_erase();
    tweaked.non_secure_erase();
    taproot::Signature {
        signature,
        sighash_type: ty,
    }
    .to_vec()
}

impl Wallet {
    /// 每个输入一项,下标和交易的输入对应,不属于本钱包的输入是空的.`network` 为空表示本机不认识这个网络.
    pub(crate) fn sign_bitcoin(&self, request: &SignRequest, network: Option<Network>) -> Result<Vec<Vec<u8>>, Error> {
        let fingerprint = self.fingerprint()?;
        if fingerprint != request.fingerprint {
            return Err(Error::Refused);
        }
        // 私钥类型只决定扩展私钥的版本前缀,派生结果和它无关.
        let mode = network.map_or(Mode::Mainnet, Network::mode);
        let prepared = prepare(network, &request.payload, fingerprint).map_err(|_| Error::Refused)?;
        self.check_own_outputs(&prepared, mode)?;
        let mut signatures = Vec::with_capacity(prepared.inputs.len());
        for (index, input) in prepared.inputs.iter().enumerate() {
            let Some(own) = &input.own else {
                signatures.push(Vec::new());
                continue;
            };
            let digest = prepared.sighash(index).map_err(|_| Error::Refused)?;
            signatures.push(self.sign_input(own, &input.prevout.script_pubkey, digest, mode)?);
        }
        Ok(signatures)
    }

    /// 声明是本钱包的输出,用私钥派生一遍.保存的公钥算不出的路径到这里才有结论.
    fn check_own_outputs(&self, prepared: &Prepared, mode: Mode) -> Result<(), Error> {
        let outputs = prepared.psbt.unsigned_tx.output.iter().zip(&prepared.outputs);
        for (out, own) in outputs {
            let Some(own) = own else {
                continue;
            };
            let mut xpriv = self.secp_key(&own.path.to_string(), mode)?;
            let derived = script_for(own.kind, &xpriv.private_key.public_key(self.secp())).ok();
            xpriv.private_key.non_secure_erase();
            if derived.as_ref() != Some(&out.script_pubkey) {
                return Err(Error::Refused);
            }
        }
        Ok(())
    }

    fn sign_input(&self, own: &OwnInput, script: &ScriptBuf, digest: [u8; 32], mode: Mode) -> Result<Vec<u8>, Error> {
        let mut xpriv = self.secp_key(&own.path.to_string(), mode)?;
        let key = &xpriv.private_key;
        let derived = script_for(own.kind, &key.public_key(self.secp())).ok();
        let signature = match own.sighash {
            _ if derived.as_ref() != Some(script) => None,
            Sighash::Ecdsa(ty) => Some(sign_ecdsa(self.secp(), key, digest, ty)),
            Sighash::Taproot(ty) => Some(sign_taproot(self.secp(), key, digest, ty)),
        };
        xpriv.private_key.non_secure_erase();
        signature.ok_or(Error::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(hex: [u8; 32]) -> SecretKey {
        SecretKey::from_slice(&hex).unwrap()
    }

    // BIP-143 的 Native P2WPKH 示例.向量的签名是 RFC 6979 的结果,所以连字节都能比.
    #[test]
    fn bip143_signature_bytes() {
        let secp = Secp256k1::new();
        let key = secret(hex_literal::hex!(
            "619c335025c7f4012e556c2a58b2506e30b8511b53ade95ea316fd8c3286feb9"
        ));
        let digest = hex_literal::hex!("c37af31116d1b27caf68aae9e3ac82f1477929014d5b917657d0eb49478cb670");
        assert_eq!(
            sign_ecdsa(&secp, &key, digest, EcdsaSighashType::All),
            hex_literal::hex!(
                "304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee01"
            )
        );
    }

    // BIP-341 的 wallet-test-vectors.json,keyPathSpending 第 0 个输入,没有 merkle root.
    #[test]
    fn bip341_signature_bytes() {
        let secp = Secp256k1::new();
        let key = secret(hex_literal::hex!(
            "6b973d88838f27366ed61c9ad6367663045cb456e28335c109e30717ae0c6baa"
        ));
        let digest = hex_literal::hex!("2514a6272f85cfa0f45eb907fcb0d121b808ed37c6ea160a5a9046ed5526d555");
        assert_eq!(
            sign_taproot(&secp, &key, digest, TapSighashType::Single),
            hex_literal::hex!(
                "ed7c1647cb97379e76892be0cacff57ec4a7102aa24296ca39af7541246d8ff14d38958d4cc1e2e478e4d4a764bbfd835b16d4e314b72937b29833060b87276c03"
            )
        );
    }

    #[test]
    fn taproot_default_has_no_trailing_byte() {
        let secp = Secp256k1::new();
        let key = secret([7; 32]);
        assert_eq!(sign_taproot(&secp, &key, [1; 32], TapSighashType::Default).len(), 64);
        assert_eq!(sign_taproot(&secp, &key, [1; 32], TapSighashType::All).len(), 65);
    }
}
