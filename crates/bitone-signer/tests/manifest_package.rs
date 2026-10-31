//! 清单包的验签。密钥是 RFC 8032 第一组测试向量，也是开发用的厂商密钥。

use bitone_core::Error;
use bitone_core::manifest::{Manifest, VENDOR_KEY};
use ed25519_dalek::{Signer, SigningKey};

const SECRET: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4, 0x44, 0x49, 0xc5,
    0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

fn package(version: u32, secret: &[u8; 32]) -> Vec<u8> {
    let mut manifest = Manifest::builtin().clone();
    manifest.version = version;
    let body = serde_json::to_vec(&manifest).unwrap();
    let mut out = SigningKey::from_bytes(secret).sign(&body).to_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

#[test]
fn dev_vendor_key_matches_the_test_secret() {
    assert_eq!(SigningKey::from_bytes(&SECRET).verifying_key().to_bytes(), VENDOR_KEY);
}

#[test]
fn valid_package_is_accepted() {
    let manifest = Manifest::from_package(&package(5, &SECRET), &VENDOR_KEY, 4).unwrap();
    assert_eq!(manifest.version, 5);
}

#[test]
fn version_cannot_go_backwards() {
    assert_eq!(
        Manifest::from_package(&package(5, &SECRET), &VENDOR_KEY, 5),
        Err(Error::StaleManifest)
    );
    assert_eq!(
        Manifest::from_package(&package(4, &SECRET), &VENDOR_KEY, 5),
        Err(Error::StaleManifest)
    );
}

#[test]
fn package_signed_by_someone_else_is_rejected() {
    assert_eq!(
        Manifest::from_package(&package(5, &[7; 32]), &VENDOR_KEY, 0),
        Err(Error::BadSignature)
    );
}

#[test]
fn tampering_is_rejected() {
    let mut tampered = package(5, &SECRET);
    let last = tampered.len() - 2;
    tampered[last] ^= 1;
    assert_eq!(
        Manifest::from_package(&tampered, &VENDOR_KEY, 0),
        Err(Error::BadSignature)
    );
    assert_eq!(Manifest::from_package(&[0; 64], &VENDOR_KEY, 0), Err(Error::Decode));
}
