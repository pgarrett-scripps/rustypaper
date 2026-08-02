//! The command line tool, exercised as a process.
//!
//! Everything else in this suite calls the library. These tests run the binary, because the
//! defects they pin are the binary's own: exit codes, and what happens to a stream the shell
//! closes underneath it.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The corpus is not committed — the PDFs are not ours to redistribute — so these skip rather
/// than fail on a fresh clone, the same way the corpus tests do.
fn paper(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join(name);
    path.is_file().then_some(path)
}

fn bin() -> PathBuf {
    // Cargo builds the binary of the crate under test before running its integration tests and
    // hands the path over in the environment.
    PathBuf::from(env!("CARGO_BIN_EXE_rustypaper"))
}

/// Reads a few lines and drops the pipe, the way `| head -n2` does.
fn run_and_close_early(args: &[&str]) -> Option<i32> {
    let mut child = Command::new(bin())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let mut stdout = child.stdout.take().expect("piped");
    let mut buf = [0u8; 256];
    let _ = stdout.read(&mut buf);
    drop(stdout);

    child.wait().expect("wait").code()
}

/// Closing the pipe early is normal shell usage and must not be reported as a failure.
///
/// `--pretty` is the case that regressed: it serialises straight to the stream, so the write
/// fails inside `serde_json` and arrives as a `serde_json::Error` wrapping the io error rather
/// than as an `io::Error`. A check that only downcast to `io::Error` missed it, and
/// `dump --pretty | head` exited 1 while every other subcommand exited 0.
#[test]
fn a_closed_pipe_is_not_a_failure() {
    let Some(pdf) = paper("unet.pdf") else {
        eprintln!("skipping: corpus/unet.pdf is absent");
        return;
    };
    let pdf = pdf.to_str().unwrap();

    for args in [
        vec!["dump", pdf, "--page", "0", "--pretty"],
        vec!["dump", pdf, "--page", "0"],
        vec!["convert", pdf],
        vec!["text", pdf],
        vec!["probe", pdf],
    ] {
        assert_eq!(
            run_and_close_early(&args),
            Some(0),
            "`{}` should exit 0 when its output pipe closes early",
            args.join(" "),
        );
    }
}

/// A missing file is a failure, so the exit code has to distinguish the two.
#[test]
fn a_real_error_still_exits_nonzero() {
    let status = Command::new(bin())
        .args(["convert", "does-not-exist.pdf"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run");
    assert!(!status.success(), "a missing file must not exit 0");
}
