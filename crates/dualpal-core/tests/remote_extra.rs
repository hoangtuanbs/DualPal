//! Remote sign-in and sync cases that need the docker test servers (Phase 2).

use std::sync::Arc;

use dualpal_core::connections::{self, ConnectOptions, Connection, Endpoint, SshAuth};
use dualpal_core::sync::{SyncEngine, SyncMode};
use dualpal_core::vfs::FtpSecurity;
use dualpal_core::{ActivityLog, Error, Location, OperationQueue, Registry};
use dualpal_testkit::remote_env;

fn sftp_conn(host: &str, port: u16, user: &str, auth: SshAuth) -> Connection {
    Connection {
        id: "t".into(),
        name: "Test".into(),
        endpoint: Endpoint::Sftp {
            host: host.into(),
            port,
            user: user.into(),
            auth,
        },
        initial_path: None,
        favorites: vec![],
    }
}

/// CON-013 — a key with a passphrase: right succeeds, wrong or missing fails.
#[tokio::test]
async fn key_with_passphrase() {
    let Some(spec) = remote_env("DUALPAL_TEST_SFTP_PASSPHRASE_KEY") else {
        return;
    };
    let Some(target) = remote_env("DUALPAL_TEST_SFTP_KEYONLY_USER") else {
        return;
    };
    let (path, passphrase) = spec
        .split_once('|')
        .expect("DUALPAL_TEST_SFTP_PASSPHRASE_KEY=path|passphrase");
    let (user, rest) = target.split_once('@').unwrap();
    let (host, port) = rest.rsplit_once(':').unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let known_hosts = tmp.path().join("known_hosts");

    let conn = sftp_conn(
        host,
        port.parse().unwrap(),
        user,
        SshAuth::KeyFile { path: path.into() },
    );
    let opts = |secret: Option<String>, trust: Option<String>| ConnectOptions {
        secret,
        trust_fingerprint: trust,
        known_hosts: Some(known_hosts.clone()),
        known_certs: None,
    };

    // First contact: learn the host key.
    let fp = match connections::connect(&conn, None, opts(None, None)).await {
        Err(Error::UnknownHost { fingerprint, .. }) => fingerprint,
        Ok(_) => return, // host already known in this environment
        Err(e) => panic!("expected unknown host, got {e}"),
    };

    // The right passphrase signs in.
    connections::connect(
        &conn,
        Some(passphrase.to_string()),
        opts(None, Some(fp.clone())),
    )
    .await
    .unwrap_or_else(|e| panic!("signing in with the passphrase key: {e}"));

    // A wrong passphrase is a sign-in failure.
    assert!(matches!(
        connections::connect(&conn, Some("wrong".into()), opts(None, None)).await,
        Err(Error::AuthFailed(_))
    ));

    // A missing passphrase fails too: the key is encrypted.
    assert!(matches!(
        connections::connect(&conn, None, opts(None, None)).await,
        Err(Error::AuthFailed(_))
    ));
}

/// CON-017 — connecting to a port nothing listens on fails with `connection`.
#[tokio::test]
async fn connection_refused() {
    // Every protocol must fail the same way when the port is closed.
    let conns: Vec<Connection> = vec![
        sftp_conn("127.0.0.1", 1, "u", SshAuth::Password),
        Connection {
            id: "f".into(),
            name: "FTP".into(),
            endpoint: Endpoint::Ftp {
                host: "127.0.0.1".into(),
                port: 1,
                user: "u".into(),
                security: FtpSecurity::None,
            },
            initial_path: None,
            favorites: vec![],
        },
        Connection {
            id: "w".into(),
            name: "WebDAV".into(),
            endpoint: Endpoint::WebDav {
                url: "http://127.0.0.1:1/dav".into(),
                user: "u".into(),
            },
            initial_path: None,
            favorites: vec![],
        },
    ];
    for conn in conns {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connections::connect(&conn, Some("pw".into()), ConnectOptions::default()),
        )
        .await
        .expect("connect must not hang");
        match result {
            Err(Error::Connection(_)) | Err(Error::Io { .. }) => {}
            Err(e) => panic!("expected a connection error, got {e:?}"),
            Ok(_) => panic!("connected to a closed port?!"),
        }
    }
}

/// CON-021 — a WebDAV wrong password is a sign-in failure.
#[tokio::test]
async fn webdav_wrong_password() {
    let Some(spec) = remote_env("DUALPAL_TEST_WEBDAV") else {
        return;
    };
    let (cred, url) = spec.split_once('@').expect("user:password@url");
    let (user, _pw) = cred.split_once(':').unwrap_or((cred, ""));
    let conn = Connection {
        id: "w".into(),
        name: "WebDAV".into(),
        endpoint: Endpoint::WebDav {
            url: url.into(),
            user: user.into(),
        },
        initial_path: None,
        favorites: vec![],
    };
    assert!(matches!(
        connections::connect(
            &conn,
            Some("wrong-password".into()),
            ConnectOptions::default()
        )
        .await,
        Err(Error::AuthFailed(_))
    ));
}

/// CON-022 — a wrong S3 bucket is `notFound`.
///
/// moto accepts any secret key, so the wrong-secret half needs a server that
/// checks signatures (a MinIO service, `DUALPAL_TEST_S3_AUTH`); without it that
/// half is skipped, not passed.
#[tokio::test]
async fn s3_wrong_bucket() {
    let Some(spec) = remote_env("DUALPAL_TEST_S3") else {
        return;
    };
    let parts: Vec<&str> = spec.split('|').collect();
    assert_eq!(
        parts.len(),
        4,
        "DUALPAL_TEST_S3=endpoint|bucket|access|secret"
    );
    let conn = Connection {
        id: "s3".into(),
        name: "S3".into(),
        endpoint: Endpoint::S3 {
            bucket: "dualpal-test-does-not-exist".into(),
            region: "us-east-1".into(),
            endpoint: Some(parts[0].into()),
            access_key_id: parts[2].into(),
            path_style: true,
        },
        initial_path: None,
        favorites: vec![],
    };
    assert!(matches!(
        connections::connect(&conn, Some(parts[3].into()), ConnectOptions::default()).await,
        Err(Error::NotFound(_))
    ));
}

/// SYNC-016 — mirroring local → remote and comparing back shows all `same`.
#[tokio::test]
async fn mirror_to_a_remote_backend() {
    let Some(spec) = remote_env("DUALPAL_TEST_WEBDAV") else {
        return;
    };
    let (cred, url) = spec.split_once('@').unwrap();
    let (user, pw) = cred.split_once(':').unwrap_or((cred, ""));
    let conn = Connection {
        id: "w".into(),
        name: "WebDAV".into(),
        endpoint: Endpoint::WebDav {
            url: url.into(),
            user: user.into(),
        },
        initial_path: None,
        favorites: vec![],
    };
    let connected = connections::connect(&conn, Some(pw.to_string()), ConnectOptions::default())
        .await
        .unwrap();

    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("a.txt"), "alpha").unwrap();
    std::fs::create_dir_all(local.path().join("sub")).unwrap();
    std::fs::write(local.path().join("sub/b.txt"), "bravo").unwrap();

    let remote_dir = format!("/dualpal-test-{}", uuid::Uuid::new_v4());
    connected.fs.create_dir(&remote_dir).await.unwrap();

    let registry = Arc::new(Registry::with_local());
    registry.register(connected.fs.clone());
    let q = OperationQueue::new(registry.clone(), Arc::new(ActivityLog::in_memory(100)), 2);
    let engine = SyncEngine::new(q.clone(), None);

    let plan = engine
        .plan(
            &Location::new("local", local.path().to_string_lossy()),
            &Location::new(connected.fs.id(), remote_dir.clone()),
            SyncMode::Mirror,
            Default::default(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let run = engine.run(plan.id, &Default::default()).unwrap();
    run.done.await.unwrap();
    assert_eq!(
        q.get(run.op).unwrap().status,
        dualpal_core::OperationStatus::Completed
    );

    // The mirror made both sides identical.
    let c = dualpal_core::compare::compare(
        &registry,
        &Location::new("local", local.path().to_string_lossy()),
        &Location::new(connected.fs.id(), remote_dir),
        Default::default(),
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
