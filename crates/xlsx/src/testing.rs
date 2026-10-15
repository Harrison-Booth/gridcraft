//! Test support: builds password-encrypted workbooks in code, so tests here and in the crates
//! above (engine, UI) never need an encrypted file on disk. Compiled for this crate's tests and,
//! elsewhere, only with the `test-support` feature (a dev-dependency feature, never shipped).
//!
//! Salts and keys are fixed so the output is deterministic. This is not an encryptor for real
//! use: a fixed key protects nothing. Agile files come from the production encryptor
//! (`crate::encrypt`) given these fixed secrets instead of random ones.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use aes::cipher::{BlockCipherEncrypt, KeyInit};

use crate::crypto::{AgileSecrets, F_AES, F_CRYPTOAPI, Hash, encrypt_agile_with, standard_key};

/// The spin count `encrypt_workbook` uses: Excel's default.
pub const SPIN_COUNT: u32 = crate::crypto::ENCRYPT_SPIN_COUNT;

const PASSWORD_SALT: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const KEY_DATA_SALT: [u8; 16] = [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31];
const SECRET_KEY: [u8; 32] =
    [64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95];
const VERIFIER: [u8; 16] = [32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47];
/// The Agile secrets, fixed so the output is deterministic.
const FIXED: AgileSecrets =
    AgileSecrets { key_data_salt: KEY_DATA_SALT, password_salt: PASSWORD_SALT, key: SECRET_KEY, verifier: VERIFIER, hmac_key: [7; 64] };

/// `package` (normally an `.xlsx` zip) as Excel stores it with a password: Agile encryption
/// (AES-256, SHA-512) inside a compound file.
pub fn encrypt_workbook(package: &[u8], password: &str) -> Vec<u8> {
    let (info, encrypted) = encrypt_agile(package, password, SPIN_COUNT);
    compound_file(&[("EncryptionInfo", &info), ("EncryptedPackage", &encrypted)])
}

/// Whether an Agile workbook's data-integrity HMAC matches its package (`None`: it has none),
/// checked with the right password, as Excel checks it.
pub fn integrity_ok(file: &[u8], password: &str) -> Option<bool> {
    crate::crypto::agile_integrity_ok(file, password).unwrap()
}

/// The same with Standard encryption (AES-128, SHA-1), as Excel 2007 wrote it.
pub fn encrypt_workbook_standard(package: &[u8], password: &str) -> Vec<u8> {
    let (info, encrypted) = encrypt_standard(package, password);
    compound_file(&[("EncryptionInfo", &info), ("EncryptedPackage", &encrypted)])
}

/// A version 3 compound file (512-byte sectors) holding `streams` under the root, laid out as
/// Excel lays it out: streams under 4096 bytes in the mini stream, larger ones in regular
/// sectors. Written by the production writer, which adds DIFAT sectors past about 7 MB.
pub fn compound_file(streams: &[(&str, &[u8])]) -> Vec<u8> {
    crate::cfb::write(streams).unwrap()
}

fn encrypt_block(key: &[u8], block: &mut [u8; 16]) {
    let b = aes::Block::cast_from_core_mut(block);
    match key.len() {
        16 => aes::Aes128::new_from_slice(key).unwrap().encrypt_block(b),
        32 => aes::Aes256::new_from_slice(key).unwrap().encrypt_block(b),
        n => panic!("no {n}-byte AES in the test encryptor"),
    }
}

fn pad(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v.resize(data.len().div_ceil(16) * 16, 0);
    v
}

fn encrypt_ecb(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut v = pad(data);
    v.as_chunks_mut::<16>().0.iter_mut().for_each(|b| encrypt_block(key, b));
    v
}

fn package(size: usize, body: Vec<u8>) -> Vec<u8> {
    let mut p = (size as u64).to_le_bytes().to_vec();
    p.extend(body);
    p
}

/// The Agile XML descriptor: AES-256, SHA-512, a password encryptor salted with 0..16, and a
/// data-integrity HMAC over an empty package.
#[cfg(test)]
pub(crate) fn agile_info(password: &str, spin: u32) -> String {
    crate::crypto::agile_xml(password, spin, &FIXED, &package(0, vec![])).unwrap()
}

/// `EncryptionInfo` for an Agile descriptor: version 4.4, reserved 0x40, then the XML.
#[cfg(test)]
pub(crate) fn with_agile_header(xml: &str) -> Vec<u8> {
    let mut info = vec![4, 0, 4, 0, 0x40, 0, 0, 0];
    info.extend_from_slice(xml.as_bytes());
    info
}

/// `EncryptionInfo` and `EncryptedPackage` for `plain`, Agile (AES-256, SHA-512), with the
/// fixed secrets.
pub(crate) fn encrypt_agile(plain: &[u8], password: &str, spin: u32) -> (Vec<u8>, Vec<u8>) {
    encrypt_agile_with(plain, password, spin, &FIXED).unwrap()
}

/// `EncryptionInfo` and `EncryptedPackage` for `plain`, Standard (AES-128, SHA-1).
pub(crate) fn encrypt_standard(plain: &[u8], password: &str) -> (Vec<u8>, Vec<u8>) {
    let key = standard_key(password, &PASSWORD_SALT, 16);
    let mut header = vec![];
    for v in [F_CRYPTOAPI | F_AES, 0, 0x660E, 0x8004, 128, 0x18, 0, 0] {
        header.extend_from_slice(&u32::to_le_bytes(v));
    }
    header.extend("\0".encode_utf16().flat_map(u16::to_le_bytes));
    let mut info = vec![4, 0, 2, 0];
    info.extend_from_slice(&(F_CRYPTOAPI | F_AES).to_le_bytes());
    info.extend_from_slice(&(header.len() as u32).to_le_bytes());
    info.extend(header);
    info.extend_from_slice(&16u32.to_le_bytes());
    info.extend(PASSWORD_SALT);
    info.extend(encrypt_ecb(&key, &VERIFIER));
    info.extend_from_slice(&20u32.to_le_bytes());
    info.extend(encrypt_ecb(&key, &Hash::Sha1.digest(&[&VERIFIER])));
    (info, package(plain.len(), encrypt_ecb(&key, plain)))
}
