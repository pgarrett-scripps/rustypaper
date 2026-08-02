//! Malformed and hostile input.
//!
//! This converter is meant to run unattended over archives, where a proportion of files are
//! truncated, corrupt, encrypted or simply not PDFs. Every one of those must produce an error,
//! never a panic and never a hang: one bad file in ten thousand must not take down the batch.

use std::io::Write;

/// Writes `bytes` to a temporary file and returns its path.
fn scratch(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("rustypdf-robustness-{name}"));
    let mut file = std::fs::File::create(&path).expect("create scratch file");
    file.write_all(bytes).expect("write scratch file");
    path
}

fn corpus(name: &str) -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join(name);
    path.exists().then_some(path)
}

#[test]
fn garbage_is_rejected_without_panicking() {
    let cases: [(&str, Vec<u8>); 6] = [
        ("empty", Vec::new()),
        ("text", b"this is not a PDF at all".to_vec()),
        ("header-only", b"%PDF-1.7\n".to_vec()),
        // A plausible header followed by nonsense.
        ("bad-body", {
            let mut v = b"%PDF-1.4\n".to_vec();
            v.extend(std::iter::repeat_n(0xAB, 4096));
            v
        }),
        ("nul-bytes", vec![0u8; 1024]),
        // Something that looks like a different format entirely.
        ("png", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec()),
    ];

    for (name, bytes) in cases {
        let path = scratch(name, &bytes);
        let result = rustypdf::convert(&path);
        assert!(
            result.is_err(),
            "{name}: garbage was accepted as a document"
        );
        // The error must say something useful, not be empty.
        assert!(
            !result.unwrap_err().to_string().is_empty(),
            "{name}: empty error"
        );
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn a_missing_file_is_an_error_not_a_panic() {
    let missing = std::env::temp_dir().join("rustypdf-does-not-exist-9d3f.pdf");
    let _ = std::fs::remove_file(&missing);
    assert!(rustypdf::convert(&missing).is_err());
}

/// A file truncated part-way through is the commonest real corruption.
#[test]
fn truncated_documents_do_not_panic() {
    let Some(path) = corpus("unet.pdf") else {
        eprintln!("skipping: corpus absent");
        return;
    };
    let whole = std::fs::read(&path).expect("read corpus file");

    // Cut at a spread of points, including inside the cross-reference table at the end.
    for fraction in [1, 2, 5, 10, 25, 50, 75, 95, 99] {
        let cut = whole.len() * fraction / 100;
        let path = scratch(&format!("truncated-{fraction}"), &whole[..cut]);
        // Either outcome is acceptable; what matters is that neither panics.
        let _ = rustypdf::convert(&path);
        let _ = std::fs::remove_file(&path);
    }
}

/// A single flipped byte anywhere in a valid document must not bring the process down.
#[test]
fn bit_flips_do_not_panic() {
    let Some(path) = corpus("unet.pdf") else {
        eprintln!("skipping: corpus absent");
        return;
    };
    let whole = std::fs::read(&path).expect("read corpus file");

    // A deterministic spread of offsets, so a failure is reproducible.
    for step in 1..=40u64 {
        let offset = (step * 2_654_435_761 % whole.len() as u64) as usize;
        let mut damaged = whole.clone();
        damaged[offset] ^= 0xFF;
        let path = scratch(&format!("flipped-{step}"), &damaged);
        let _ = rustypdf::convert(&path);
        let _ = std::fs::remove_file(&path);
    }
}

/// A page-range request outside the document must be an error.
#[test]
fn out_of_range_pages_are_rejected() {
    use rustypdf::backend::{open as open_backend, PageSource};

    let Some(path) = corpus("unet.pdf") else {
        eprintln!("skipping: corpus absent");
        return;
    };
    let backend = open_backend(&path).expect("open");
    let mut fonts = rustypdf::ir::FontTable::new();
    assert!(backend.page(usize::MAX, &mut fonts).is_err());
    assert!(backend.page(backend.page_count(), &mut fonts).is_err());
}
