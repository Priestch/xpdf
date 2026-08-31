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

/// hayro-tests password fixtures: open locked, then authenticate with the known
/// user password and resolve page 0. Proves decryptor install + cache clear.
#[test]
fn hayro_password_fixtures_authenticate_and_resolve() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hayro/hayro-tests/pdfs/custom");
    let cases: &[(&str, &str)] = &[
        ("password_1.pdf", "supersecret"),
        ("password_2.pdf", "samplefiles"),
        ("password_encrypted_aes_128.pdf", "testpw"),
        ("password_encrypted_aes_256.pdf", "testpw"),
        ("password_encrypted_rc4_40.pdf", "testpw"),
        ("password_encrypted_rc4_128.pdf", "testpw"),
    ];
    for &(name, password) in cases {
        let path = root.join(name);
        if !path.exists() {
            eprintln!("skip: {} not present", path.display());
            continue;
        }
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));
        let mut doc =
            PDFDocument::open(data).unwrap_or_else(|e| panic!("{name}: open locked doc: {e}"));
        assert!(doc.is_encrypted(), "{name} should report encrypted");
        assert!(
            doc.authenticate_with_password(password.as_bytes())
                .unwrap_or_else(|e| panic!("{name}: authenticate: {e}")),
            "{name}: password {password:?} must authenticate"
        );
        let pages = doc
            .page_count()
            .unwrap_or_else(|e| panic!("{name}: page_count after auth: {e}"));
        assert!(pages >= 1, "{name}: expected >=1 page, got {pages}");
        doc.get_page(0)
            .unwrap_or_else(|e| panic!("{name}: get_page(0) after auth: {e}"));
    }
}

#[test]
fn password_2_extracts_body_text() {
    use pdf_x_core::core::parser::PDFObject;
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hayro/hayro-tests/pdfs/custom/password_2.pdf");
    if !path.exists() {
        return;
    }
    let data = std::fs::read(&path).expect("read");
    let mut doc = PDFDocument::open(data).expect("open");
    assert!(
        doc.authenticate_with_password(b"samplefiles")
            .expect("auth")
    );
    let text = doc
        .extract_text_from_page_as_string(0)
        .expect("extract text");
    eprintln!("password_2 text:\n{text}");
    assert!(
        text.contains("Introduction to Password Protection"),
        "title missing: {text:?}"
    );
    assert!(
        text.contains("Why Use Password Protection")
            || text.contains("unauthorized")
            || text.contains("Benefits of Securing"),
        "body text missing — content stream may abort early: {text:?}"
    );
}
