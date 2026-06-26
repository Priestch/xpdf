//! End-to-end decryption tests across encryption revisions.
//!
//! - V=5/R=6 AES-256, empty user password, catalog in an Object Stream
//!   (open → fetch-decrypt wiring, the catalog-in-objstm case).
//! - V=2/R=3 RC4-128, empty user password (legacy key derivation + U-check).
//! - V=2/R=3 RC4-128, password "test" (authenticate_with_password installs decryptor).
//! - V=4/R=4 AES-128, password "test" (AES-128 object-key path).

use pdf_x_core::core::PDFDocument;
use std::path::PathBuf;

fn pdfs() -> PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("pdfs")
}

#[test]
fn v5_aes256_empty_password_opens() {
    let data = std::fs::read(pdfs().join("catalog-in-objstm-aes.pdf")).expect("fixture exists");
    let mut doc = PDFDocument::open(data).expect("encrypted PDF opens");

    assert!(doc.is_encrypted());
    assert!(
        doc.authenticate_with_password(b"").expect("auth"),
        "empty user password must authenticate"
    );
    // Catalog lives in an ObjStm — must be decrypted on fetch.
    assert_eq!(doc.page_count().expect("page_count"), 1);
    doc.get_page(0)
        .expect("page 0 resolves (ObjStm decrypted on fetch)");
}

#[test]
fn v2_rc4_empty_password_opens() {
    let data = std::fs::read(pdfs().join("rc4-128-empty.pdf")).expect("fixture exists");
    let mut doc = PDFDocument::open(data).expect("encrypted PDF opens");

    assert!(doc.is_encrypted());
    // Empty user password is auto-authenticated during open; authenticate again to be sure.
    assert!(doc.authenticate_with_password(b"").expect("auth"));
    assert_eq!(doc.page_count().expect("page_count"), 3);
}

#[test]
fn v2_rc4_password_installs_decryptor() {
    // Encrypted with user password "test" — open() can't auto-auth (empty pw fails), so the
    // document opens "locked"; authenticate_with_password("test") must derive the key AND
    // install the decryptor so objects subsequently decrypt.
    let data = std::fs::read(pdfs().join("rc4-128-test.pdf")).expect("fixture exists");
    let mut doc = PDFDocument::open(data).expect("encrypted PDF opens");

    assert!(doc.is_encrypted());
    assert!(
        doc.authenticate_with_password(b"test").expect("auth"),
        "user password 'test' must authenticate"
    );
    assert_eq!(doc.page_count().expect("page_count"), 3);
    doc.get_page(0)
        .expect("page resolves after decryptor install");
}

#[test]
fn v4_aes128_password_installs_decryptor() {
    let data = std::fs::read(pdfs().join("aes-128-test.pdf")).expect("fixture exists");
    let mut doc = PDFDocument::open(data).expect("encrypted PDF opens");

    assert!(doc.is_encrypted());
    assert!(doc.authenticate_with_password(b"test").expect("auth"));
    assert_eq!(doc.page_count().expect("page_count"), 3);
    doc.get_page(0)
        .expect("page resolves after AES-128 decryptor install");
}
