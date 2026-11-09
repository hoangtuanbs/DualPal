//! The VFS contract suite against the real protocol servers (Phase 2, L3).
//!
//! Each backend is one `rstest` case. A backend whose `DUALPAL_TEST_*` variable
//! is missing is skipped — unless `DUALPAL_REQUIRE_REMOTE=1`, in which case
//! `remote_env` panics (PREP-07), so CI cannot silently turn this suite green.
//!
//! Run with:
//!   docker compose -f docker/test-servers/compose.yaml up -d --build --wait
//!   eval "$(scripts/ci/remote-env.sh)"
//!   cargo nextest run -p dualpal-core --test contract_remote

use std::sync::Arc;

use dualpal_core::vfs::FtpSecurity;
use dualpal_testkit::remote::{ftp_from_env, s3_from_env, sftp_from_env, webdav_from_env, TestFs};
use dualpal_testkit::{run_contract, unique_base, Caps};
use rstest::rstest;

fn caps_sftp() -> Caps {
    Caps {
        modes: true,
        set_mtime: true,
        symlinks: true,
        trash: false,
        resumable: true,
        atomic_writes: false,
        empty_dirs: true,
        case_sensitive: true,
    }
}

fn caps_ftp() -> Caps {
    Caps {
        modes: false,
        set_mtime: true, // pyftpdlib supports MFMT
        symlinks: false,
        trash: false,
        resumable: true,
        atomic_writes: false,
        empty_dirs: true,
        case_sensitive: true,
    }
}

fn caps_webdav() -> Caps {
    Caps {
        modes: false,
        set_mtime: false,
        symlinks: false,
        trash: false,
        resumable: false,
        atomic_writes: false,
        empty_dirs: true,
        case_sensitive: true,
    }
}

fn caps_s3() -> Caps {
    Caps {
        modes: false,
        set_mtime: false,
        symlinks: false,
        trash: false,
        resumable: false,
        atomic_writes: true,
        empty_dirs: true,
        case_sensitive: true,
    }
}

async fn run(t: TestFs, caps: Caps) {
    let base = unique_base(&t.fs, &t.start_path).await;
    run_contract(t.fs.clone(), base.path(), caps).await;
}

#[rstest]
#[case("sftp")]
#[case("ftp")]
#[case("ftps")]
#[case("ftps-implicit")]
#[case("webdav")]
#[case("s3")]
#[tokio::test]
async fn contract_suite(#[case] backend: &str) {
    match backend {
        "sftp" => {
            let Some(t) = sftp_from_env().await else {
                return;
            };
            run(t, caps_sftp()).await;
        }
        "ftp" => {
            let Some(t) = ftp_from_env("DUALPAL_TEST_FTP", FtpSecurity::None).await else {
                return;
            };
            run(t, caps_ftp()).await;
        }
        "ftps" => {
            let Some(t) = ftp_from_env("DUALPAL_TEST_FTPS", FtpSecurity::Explicit).await else {
                return;
            };
            run(t, caps_ftp()).await;
        }
        "ftps-implicit" => {
            let Some(t) = ftp_from_env("DUALPAL_TEST_FTPS_IMPLICIT", FtpSecurity::Implicit).await
            else {
                return;
            };
            run(t, caps_ftp()).await;
        }
        "webdav" => {
            let Some(t) = webdav_from_env().await else {
                return;
            };
            run(t, caps_webdav()).await;
        }
        "s3" => {
            let Some(t) = s3_from_env().await else { return };
            run(t, caps_s3()).await;
        }
        other => panic!("unknown backend {other}"),
    }
}

/// CMP-012 — an identical tree compares as all `same` on every backend.
#[rstest]
#[case("sftp")]
#[case("ftp")]
#[case("webdav")]
#[case("s3")]
#[tokio::test]
async fn compare_identical_trees(#[case] backend: &str) {
    let Some(t) = backend_fs(backend).await else {
        return;
    };
    let fs = t.fs.clone();
    let base = unique_base(&fs, &t.start_path).await;
    // Two identical folders: the contract just proved create/write/stat work.
    let l = fs.join(base.path(), "left");
    let r = fs.join(base.path(), "right");
    fs.create_dir(&l).await.unwrap();
    fs.create_dir(&r).await.unwrap();
    for (dir, name, content) in [
        (&l, "a.txt", "alpha"),
        (&l, "b.txt", "bravo"),
        (&r, "a.txt", "alpha"),
        (&r, "b.txt", "bravo"),
    ] {
        let mut w = fs.open_write(&fs.join(dir, name)).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut w, content.as_bytes())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::shutdown(&mut w).await.unwrap();
    }
    let registry = Arc::new(dualpal_core::Registry::with_local());
    registry.register(fs.clone());
    let c = dualpal_core::compare::compare(
        &registry,
        &dualpal_core::Location::new(fs.id(), l),
        &dualpal_core::Location::new(fs.id(), r),
        dualpal_core::compare::CompareOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert!(
        c.items
            .iter()
            .all(|i| i.state == dualpal_core::compare::DiffState::Same),
        "{:?}",
        c.items
    );
}

async fn backend_fs(backend: &str) -> Option<TestFs> {
    match backend {
        "sftp" => sftp_from_env().await,
        "ftp" => ftp_from_env("DUALPAL_TEST_FTP", FtpSecurity::None).await,
        "webdav" => webdav_from_env().await,
        "s3" => s3_from_env().await,
        _ => None,
    }
}
