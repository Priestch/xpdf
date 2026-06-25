//! End-to-end decryption tests: an encrypted PDF (V=5/R=6 AES-256, empty user
//! password, catalog inside an Object Stream) must open with the empty password and
//! resolve its objects — proving the open → fetch-decrypt wiring.

use pdf_x_core::core::PDFDocument;

fn fixture() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("pdfs")
        .join("catalog-in-objstm-aes.pdf")
}

#[test]
fn encrypted_pdf_opens_with_empty_password() {
    let data = std::fs::read(fixture()).expect("fixture exists");
    let mut doc = PDFDocument::open(data).expect("encrypted PDF opens");

    assert!(doc.is_encrypted(), "document should report as encrypted");
    assert!(
        doc.authenticate_with_password(b"").expect("auth"),
        "empty user password must authenticate and derive the key"
    );

    // The catalog lives in an ObjStm and must be decrypted on fetch — page tree resolves.
    assert_eq!(doc.page_count().expect("page_count"), 1);
    doc.get_page(0).expect("page 0 resolves (ObjStm decrypted on fetch)");
}
