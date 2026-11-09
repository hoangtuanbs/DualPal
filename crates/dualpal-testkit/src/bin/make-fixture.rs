//! Create a fixture tree for the TypeScript E2E suites and shell scripts.
//!
//! `cargo run -p dualpal-testkit --bin make-fixture -- <dir> [standard|small|many N]`

use std::path::PathBuf;

use dualpal_testkit::{many_files, small_tree, standard_tree};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| usage("a folder is required"));
    let kind = args.next().unwrap_or_else(|| "standard".to_string());
    let root = match kind.as_str() {
        "standard" => standard_tree(&dir),
        "small" => small_tree(&dir),
        "many" => {
            let n = args
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| usage("many needs a file count"));
            many_files(&dir, n)
        }
        other => usage(&format!("unknown fixture kind {other:?}")),
    };
    println!("{}", root.display());
}

fn usage(message: &str) -> ! {
    eprintln!("{message}");
    eprintln!("usage: make-fixture <dir> [standard|small|many N]");
    std::process::exit(2);
}
