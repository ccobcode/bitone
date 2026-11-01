//! 清单包的验签.这里的私钥写在代码里,只用来测试规则.

use bitone_core::Error;
use bitone_core::manifest::{Manifest, VENDOR_KEY};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{DerSignature, SigningKey};
use p256::pkcs8::EncodePublicKey;

const SECRET: [u8; 32] = [3; 32];

fn key(secret: &[u8; 32]) -> SigningKey {
    SigningKey::from_slice(secret).unwrap()
}

fn public() -> Vec<u8> {
    key(&SECRET).verifying_key().to_public_key_der().unwrap().into_vec()
}

fn signed(manifest: &Manifest, secret: &[u8; 32]) -> Vec<u8> {
    let body = serde_json::to_vec(manifest).unwrap();
    let signature: DerSignature = key(secret).sign(&body);
    let mut out = signature.as_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

fn manifest(version: u32) -> Manifest {
    let mut manifest = Manifest::builtin().clone();
    manifest.version = version;
    manifest
}

fn package(version: u32, secret: &[u8; 32]) -> Vec<u8> {
    signed(&manifest(version), secret)
}

/// 私钥公开的密钥不能当厂商密钥,否则任何人都能发清单.走到验签这一步也说明厂商公钥能解析.
#[test]
fn vendor_key_is_not_the_test_key() {
    assert_ne!(public(), VENDOR_KEY);
    assert_eq!(
        Manifest::from_package(&package(5, &SECRET), &VENDOR_KEY, 0),
        Err(Error::BadSignature)
    );
}

#[test]
fn valid_package_is_accepted() {
    let manifest = Manifest::from_package(&package(5, &SECRET), &public(), 4).unwrap();
    assert_eq!(manifest.version, 5);
}

#[test]
fn version_cannot_go_backwards() {
    assert_eq!(
        Manifest::from_package(&package(5, &SECRET), &public(), 5),
        Err(Error::StaleManifest)
    );
    assert_eq!(
        Manifest::from_package(&package(4, &SECRET), &public(), 5),
        Err(Error::StaleManifest)
    );
}

#[test]
fn package_signed_by_someone_else_is_rejected() {
    assert_eq!(
        Manifest::from_package(&package(5, &[7; 32]), &public(), 0),
        Err(Error::BadSignature)
    );
}

#[test]
fn tampering_is_rejected() {
    let mut tampered = package(5, &SECRET);
    let last = tampered.len() - 2;
    tampered[last] ^= 1;
    assert_eq!(
        Manifest::from_package(&tampered, &public(), 0),
        Err(Error::BadSignature)
    );
    assert_eq!(Manifest::from_package(&[0; 64], &public(), 0), Err(Error::Decode));
}

/// 签名长度来自包本身,长度和实际内容对不上时不能越界.
#[test]
fn truncated_packages_are_rejected() {
    let whole = package(5, &SECRET);
    for length in [0, 1, 2, 16, usize::from(whole[1]) + 1] {
        assert_eq!(
            Manifest::from_package(&whole[..length], &public(), 0),
            Err(Error::Decode),
            "{length}"
        );
    }
}

/// 签名通过就整体覆盖,内置资产和原生币也一样.
#[test]
fn a_signed_package_overrides_the_builtin_manifest() {
    let mut changed = manifest(5);
    let native = changed
        .assets
        .iter_mut()
        .find(|a| a.network == "ethereum" && a.contract == "native")
        .unwrap();
    native.symbol = "NEW".into();
    native.decimals = 9;
    changed.assets.retain(|a| a.network != "polygon");

    let accepted = Manifest::from_package(&signed(&changed, &SECRET), &public(), 0).unwrap();
    let native = accepted.native("ethereum").unwrap();
    assert_eq!((native.symbol.as_str(), native.decimals), ("NEW", 9));
    assert!(accepted.native("polygon").is_none());
}

/// 升级到内置版本更高的 APK 后,手机上的旧包不能盖住新的内置清单.
#[test]
fn the_higher_version_of_builtin_and_imported_is_active() {
    let builtin = Manifest::builtin();
    assert_eq!(*Manifest::active(None, &public()), *builtin);

    let mut stale = manifest(builtin.version);
    stale.assets.clear();
    assert_eq!(*Manifest::active(Some(&signed(&stale, &SECRET)), &public()), *builtin);

    let newer = manifest(builtin.version + 1);
    assert_eq!(*Manifest::active(Some(&signed(&newer, &SECRET)), &public()), newer);
}

#[test]
fn an_imported_package_with_a_bad_signature_falls_back_to_the_builtin_list() {
    let forged = package(Manifest::builtin().version + 1, &[7; 32]);
    assert_eq!(*Manifest::active(Some(&forged), &public()), *Manifest::builtin());
}
