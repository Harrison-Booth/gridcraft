//! Encrypted workbooks end to end: compound file → streams → plain package.

use crate::testing::{compound_file, encrypt_workbook, encrypt_workbook_standard};
use crate::{DecryptError, decrypt};

#[test]
fn the_test_compound_writer_reads_back() {
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    let mid: Vec<u8> = (0..4095u32).map(|i| i as u8).collect();
    let file = compound_file(&[("Small", b"hello"), ("Big", &big), ("Empty", b""), ("Mid", &mid)]);
    let cf = crate::cfb::Compound::open(&file).unwrap();
    assert_eq!(cf.root_stream("Mid").unwrap().unwrap(), mid, "just under the cutoff: the mini stream");
    assert_eq!(cf.root_stream("Small").unwrap().unwrap(), b"hello");
    assert_eq!(cf.root_stream("big").unwrap().unwrap(), big);
    assert_eq!(cf.root_stream("Empty").unwrap().unwrap(), b"");
    assert_eq!(cf.root_stream("Missing").unwrap(), None);
}

#[test]
fn large_compound_files_grow_difat_sectors() {
    // Past 109 FAT sectors (about 7 MB) the FAT's own sector list continues in DIFAT sectors.
    let big: Vec<u8> = (0..9_000_000u32).map(|i| (i % 241) as u8).collect();
    let file = compound_file(&[("Small", b"hi"), ("Big", &big)]);
    let difat_count = u32::from_le_bytes(file[72..76].try_into().unwrap());
    assert!(difat_count >= 1, "{difat_count}");
    let cf = crate::cfb::Compound::open(&file).unwrap();
    assert_eq!(cf.root_stream("Big").unwrap().unwrap(), big);
    assert_eq!(cf.root_stream("Small").unwrap().unwrap(), b"hi");
}

#[test]
fn the_directory_is_a_balanced_tree_in_name_order() {
    let names = ["Workbook", "EncryptionInfo", "EncryptedPackage", "A", "\u{5}SummaryInformation", "zz"];
    let streams: Vec<(&str, &[u8])> = names.iter().map(|n| (*n, &b"x"[..])).collect();
    let file = compound_file(&streams);
    let cf = crate::cfb::Compound::open(&file).unwrap();
    for n in names {
        assert_eq!(cf.root_stream(n).unwrap().as_deref(), Some(&b"x"[..]), "{n}");
    }
    // The root's child is the middle name: by length, then upper case ("A" < "zz" < ... ).
    let dir = 512 * 2;
    let child = u32::from_le_bytes(file[dir + 76..dir + 80].try_into().unwrap()) as usize;
    let e = dir + child * 128;
    let len = u16::from_le_bytes(file[e + 64..e + 66].try_into().unwrap()) as usize;
    let name: Vec<u16> = file[e..e + len - 2].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16(&name).unwrap(), "EncryptionInfo");
    assert!(crate::cfb::write(&[("", b"x")]).is_err());
    assert!(crate::cfb::write(&[(&"n".repeat(32), b"x")]).is_err());
}

#[test]
fn encrypted_files_decrypt_to_the_package() {
    let package: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let agile = encrypt_workbook(&package, "Pass1234");
    assert!(crate::cfb::is_encrypted(&agile));
    assert_eq!(decrypt(&agile, "Pass1234").unwrap(), package);
    assert_eq!(decrypt(&agile, "wrong"), Err(DecryptError::WrongPassword));
    let standard = encrypt_workbook_standard(&package, "Pass1234");
    assert_eq!(decrypt(&standard, "Pass1234").unwrap(), package);
}

#[test]
fn a_compound_file_without_the_streams_is_malformed() {
    let file = compound_file(&[("EncryptionInfo", &[4, 0, 4, 0])]);
    assert!(matches!(decrypt(&file, "x"), Err(DecryptError::Malformed(_))));
    assert!(matches!(decrypt(b"not a compound file", "x"), Err(DecryptError::Malformed(_))));
}

#[test]
fn a_gridcraft_workbook_survives_encryption() {
    let b2 = gridcraft_core::CellRef::parse("B2").unwrap();
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(b2, gridcraft_core::Value::Number(42.0));
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = encrypt_workbook(&xlsx, "Pass1234");
    assert_eq!(crate::sniff(&file), crate::Format::Encrypted);
    let (back, _) = crate::read_xlsx(&decrypt(&file, "Pass1234").unwrap()).unwrap();
    assert_eq!(back.active().unwrap().value(b2), gridcraft_core::Value::Number(42.0));
}

#[test]
fn schemes_no_password_opens_are_named_before_asking_for_one() {
    use crate::check_encryption;
    use crate::testing::{agile_info, with_agile_header};
    let unsupported = |s: &str| Err(DecryptError::Unsupported(s.into()));
    assert_eq!(check_encryption(&encrypt_workbook(b"package", "pw")), Ok(()));
    assert_eq!(check_encryption(&encrypt_workbook_standard(b"package", "pw")), Ok(()));
    let file = |info: &[u8]| compound_file(&[("EncryptionInfo", info), ("EncryptedPackage", &[0; 8])]);
    assert_eq!(check_encryption(&file(&[3, 0, 2, 0, 4, 0, 0, 0])), unsupported("RC4"));
    assert_eq!(check_encryption(&file(&[4, 0, 3, 0, 0, 0, 0, 0])), unsupported("Extensible"));
    let cert = agile_info("pw", 1000).replace("keyEncryptor/password", "keyEncryptor/certificate");
    assert_eq!(check_encryption(&file(&with_agile_header(&cert))), unsupported("certificate"));
    assert!(matches!(check_encryption(&file(&[4, 0])), Err(DecryptError::Malformed(_))));
    // Rights management (IRM) keeps an EncryptedPackage but no EncryptionInfo.
    let irm = compound_file(&[("EncryptedPackage", &[0; 8])]);
    assert_eq!(check_encryption(&irm), unsupported("rights-management"));
    assert_eq!(decrypt(&irm, "pw"), Err(DecryptError::Unsupported("rights-management".into())));
}
