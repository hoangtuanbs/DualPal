//! Remote file systems behind the same `FileSystem` trait as the local disk.
//!
//! The S3 tests run against OpenDAL's in-memory service, so they need no
//! server. The SFTP test needs a real SSH server and only runs when
//! `DUALPAL_TEST_SFTP` is set to `user@host:port` with the private key in
//! `DUALPAL_TEST_SFTP_KEY` (see README).

use std::path::Path;
use std::sync::Arc;

use dualpal_core::connections::{
    self, ConnectOptions, Connection, Connections, Endpoint, MemorySecrets, SshAuth,
};
use dualpal_core::vfs::{read_text, write_text, S3Fs};
use dualpal_core::{
    ActivityLog, ConflictPolicy, EntryKind, Error, FileSystem, Location, Operation, OperationQueue,
    OperationStatus, Registry,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn put(fs: &dyn FileSystem, path: &str, content: &str) {
    let mut w = fs.open_write(path).await.unwrap();
    w.write_all(content.as_bytes()).await.unwrap();
    w.shutdown().await.unwrap();
}

async fn get(fs: &dyn FileSystem, path: &str) -> String {
    let mut r = fs.open_read(path).await.unwrap();
    let mut s = String::new();
    r.read_to_string(&mut s).await.unwrap();
    s
}

async fn names(fs: &dyn FileSystem, dir: &str) -> Vec<String> {
    let mut n: Vec<_> = fs
        .list(dir)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    n.sort();
    n
}

async fn run(q: &OperationQueue, op: Operation) {
    let id = q.enqueue(op);
    let info = q.wait(id).await.unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
}

/// Exercise a remote file system rooted at the empty folder `base`.
async fn exercise(fs: Arc<dyn FileSystem>, base: &str) {
    // Folders and files.
    let dir = fs.join(base, "docs");
    fs.create_dir(&dir).await.unwrap();
    assert!(matches!(
        fs.create_dir(&dir).await,
        Err(Error::AlreadyExists(_))
    ));
    let empty = fs.join(base, "empty");
    fs.create_dir(&empty).await.unwrap();
    assert!(fs.list(&empty).await.unwrap().is_empty());
    put(&*fs, &fs.join(&dir, "a.txt"), "alpha").await;
    put(&*fs, &fs.join(&dir, ".hidden"), "h").await;
    assert_eq!(names(&*fs, base).await, ["docs", "empty"]);
    assert_eq!(names(&*fs, &dir).await, [".hidden", "a.txt"]);

    let st = fs.stat(&fs.join(&dir, "a.txt")).await.unwrap();
    assert_eq!(st.kind, EntryKind::File);
    assert_eq!(st.size, 5);
    assert!(fs.stat(&dir).await.unwrap().is_dir());
    assert!(fs.list(&dir).await.unwrap().iter().any(|e| e.hidden));
    assert!(matches!(
        fs.stat(&fs.join(base, "nope")).await,
        Err(Error::NotFound(_))
    ));
    assert_eq!(get(&*fs, &fs.join(&dir, "a.txt")).await, "alpha");

    // Overwrite, then the quick editor path.
    put(&*fs, &fs.join(&dir, "a.txt"), "alpha 2").await;
    let t = read_text(&*fs, &fs.join(&dir, "a.txt"), 1024)
        .await
        .unwrap();
    assert_eq!(t.text, "alpha 2");
    write_text(&*fs, &fs.join(&dir, "a.txt"), "edited")
        .await
        .unwrap();
    assert_eq!(get(&*fs, &fs.join(&dir, "a.txt")).await, "edited");

    // Renames of files and whole folders.
    fs.rename(&fs.join(&dir, "a.txt"), &fs.join(&dir, "b.txt"))
        .await
        .unwrap();
    assert_eq!(names(&*fs, &dir).await, [".hidden", "b.txt"]);
    let moved = fs.join(base, "papers");
    fs.rename(&dir, &moved).await.unwrap();
    assert_eq!(names(&*fs, base).await, ["empty", "papers"]);
    assert_eq!(get(&*fs, &fs.join(&moved, "b.txt")).await, "edited");

    // Copy a local tree up, then back down, through the operation queue.
    let registry = Arc::new(Registry::with_local());
    registry.register(fs.clone());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(10)), 2);
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("tree");
    std::fs::create_dir_all(src.join("sub/deep")).unwrap();
    std::fs::write(src.join("one.txt"), "1").unwrap();
    std::fs::write(src.join("sub/deep/two.bin"), vec![7u8; 300_000]).unwrap();
    let local = |p: &Path| Location::new("local", p.to_string_lossy());
    let remote = |p: &str| Location::new(fs.id(), p);

    run(
        &q,
        Operation::Copy {
            sources: vec![local(&src)],
            destination: remote(base),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    let up = fs.join(base, "tree");
    assert_eq!(names(&*fs, &up).await, ["one.txt", "sub"]);
    assert_eq!(
        fs.stat(&fs.join(&up, "sub/deep/two.bin"))
            .await
            .unwrap()
            .size,
        300_000
    );

    let down = tmp.path().join("down");
    std::fs::create_dir(&down).unwrap();
    run(
        &q,
        Operation::Copy {
            sources: vec![remote(&up)],
            destination: local(&down),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(
        std::fs::read(down.join("tree/sub/deep/two.bin")).unwrap(),
        vec![7u8; 300_000]
    );
    assert_eq!(
        std::fs::read_to_string(down.join("tree/one.txt")).unwrap(),
        "1"
    );

    // Move within the server, then delete everything.
    run(
        &q,
        Operation::Move {
            sources: vec![remote(&up)],
            destination: remote(&moved),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(names(&*fs, &moved).await, [".hidden", "b.txt", "tree"]);
    run(
        &q,
        Operation::Delete {
            targets: vec![remote(&moved), remote(&empty)],
        },
    )
    .await;
    assert!(names(&*fs, base).await.is_empty());
}

#[tokio::test]
async fn s3_backend_works_like_a_disk() {
    let op = opendal::Operator::new(opendal::services::Memory::default())
        .unwrap()
        .finish();
    let fs: Arc<dyn FileSystem> =
        Arc::new(S3Fs::from_operator("conn:mem".into(), "Memory".into(), op));
    // The bucket root is a folder.
    assert!(fs.stat("/").await.unwrap().is_dir());
    fs.create_dir("/work").await.unwrap();
    sync_round_trip(fs.clone(), "/work").await;
    exercise(fs, "/work").await;
}

#[test]
/// CON-001
fn connections_keep_secrets_out_of_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("connections.json");
    let secrets = Arc::new(MemorySecrets::default());
    let store = Connections::open(&file, secrets.clone());
    let conn = store
        .save(
            Connection {
                id: String::new(),
                name: "Web server".into(),
                endpoint: Endpoint::Sftp {
                    host: "example.com".into(),
                    port: 22,
                    user: "deploy".into(),
                    auth: SshAuth::Password,
                },
                initial_path: None,
                favorites: vec![],
            },
            Some("hunter2".into()),
        )
        .unwrap();
    assert!(!conn.id.is_empty());
    assert_eq!(store.secret(&conn.id).unwrap().as_deref(), Some("hunter2"));
    let json = std::fs::read_to_string(&file).unwrap();
    assert!(!json.contains("hunter2"));
    assert!(json.contains("\"method\": \"password\""));

    // Favorites inside the connection.
    store
        .add_favorite(
            &conn.id,
            connections::RemoteFavorite {
                name: "Logs".into(),
                path: "/var/log/".into(),
            },
        )
        .unwrap();
    let reopened = Connections::open(&file, secrets.clone());
    let c = reopened.get(&conn.id).unwrap();
    assert_eq!(c.favorites[0].path, "/var/log");

    // Saving without favorites keeps them; removing deletes the secret.
    let mut edited = c.clone();
    edited.favorites.clear();
    edited.name = "Web".into();
    let saved = reopened.save(edited, None).unwrap();
    assert_eq!(saved.favorites.len(), 1);
    assert_eq!(
        reopened.secret(&conn.id).unwrap().as_deref(),
        Some("hunter2")
    );
    reopened.remove(&conn.id).unwrap();
    assert!(reopened.list().is_empty());
    assert_eq!(secrets.get_secret(&conn.id), None);

    // Invalid connections are refused.
    let bad = Connection {
        id: String::new(),
        name: "x".into(),
        endpoint: Endpoint::S3 {
            bucket: "".into(),
            region: "".into(),
            endpoint: None,
            access_key_id: "".into(),
            path_style: false,
        },
        initial_path: None,
        favorites: vec![],
    };
    assert!(matches!(reopened.save(bad, None), Err(Error::Invalid(_))));
}

trait GetSecret {
    fn get_secret(&self, id: &str) -> Option<String>;
}
impl GetSecret for MemorySecrets {
    fn get_secret(&self, id: &str) -> Option<String> {
        use dualpal_core::connections::SecretStore;
        self.get(id).unwrap()
    }
}

/// Two-way sync a local folder with `base/sync` on `fs`, then check that
/// the next sync has nothing to do even though the server keeps its own
/// dates, and that a local edit goes up as a single step.
async fn sync_round_trip(fs: Arc<dyn FileSystem>, base: &str) {
    use dualpal_core::compare::CompareOptions;
    use dualpal_core::sync::{SyncAction, SyncEngine, SyncMode};
    use std::collections::HashMap;
    use tokio_util::sync::CancellationToken;

    let registry = Arc::new(Registry::with_local());
    registry.register(fs.clone());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(10)), 2);
    let engine = SyncEngine::new(q.clone(), None);
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(local.path().join("docs")).unwrap();
    std::fs::write(local.path().join("docs/a.txt"), "alpha").unwrap();
    std::fs::write(local.path().join("b.txt"), "beta").unwrap();
    let remote_dir = fs.join(base, "sync");
    fs.create_dir(&remote_dir).await.unwrap();
    put(&*fs, &fs.join(&remote_dir, "c.txt"), "from the server").await;
    let l = Location::new("local", local.path().to_string_lossy());
    let r = Location::new(fs.id(), remote_dir.clone());
    let plan = |mode| {
        let (engine, l, r) = (engine.clone(), l.clone(), r.clone());
        async move {
            engine
                .plan(
                    &l,
                    &r,
                    mode,
                    CompareOptions::default(),
                    CancellationToken::new(),
                    None,
                )
                .await
                .unwrap()
        }
    };
    let run = |p: dualpal_core::sync::SyncPreview| {
        let (engine, q) = (engine.clone(), q.clone());
        async move {
            let run = engine.run(p.id, &HashMap::new()).unwrap();
            run.done.await.unwrap();
            let info = q.get(run.op).unwrap();
            assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
        }
    };
    run(plan(SyncMode::TwoWay).await).await;
    assert_eq!(
        get(&*fs, &fs.join(&remote_dir, "docs/a.txt")).await,
        "alpha"
    );
    assert_eq!(
        std::fs::read_to_string(local.path().join("c.txt")).unwrap(),
        "from the server"
    );
    let again = plan(SyncMode::TwoWay).await;
    assert!(again.steps.is_empty(), "{:?}", again.steps);

    std::fs::write(local.path().join("b.txt"), "beta, edited").unwrap();
    let edit = plan(SyncMode::TwoWay).await;
    let steps: Vec<_> = edit
        .steps
        .iter()
        .map(|s| (s.rel.as_str(), s.action))
        .collect();
    assert_eq!(steps, vec![("b.txt", SyncAction::CopyToRight)]);
    run(edit).await;
    assert_eq!(
        get(&*fs, &fs.join(&remote_dir, "b.txt")).await,
        "beta, edited"
    );
    assert!(plan(SyncMode::TwoWay).await.steps.is_empty());

    let id = q.enqueue(Operation::Delete {
        targets: vec![Location::new(fs.id(), remote_dir)],
    });
    assert_eq!(q.wait(id).await.unwrap().status, OperationStatus::Completed);
}

/// `DUALPAL_TEST_SFTP=user@host:port`, `DUALPAL_TEST_SFTP_KEY=/path/to/key`.
#[tokio::test]
/// CON-009
async fn sftp_backend_against_a_real_server() {
    let Some(target) = dualpal_testkit::remote_env("DUALPAL_TEST_SFTP") else {
        return;
    };
    let key = dualpal_testkit::remote_env("DUALPAL_TEST_SFTP_KEY").expect("DUALPAL_TEST_SFTP_KEY");
    let (user, rest) = target.split_once('@').unwrap();
    let (host, port) = rest.rsplit_once(':').unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let known_hosts = tmp.path().join("known_hosts");

    let conn = Connection {
        id: "t".into(),
        name: "Test server".into(),
        endpoint: Endpoint::Sftp {
            host: host.into(),
            port: port.parse().unwrap(),
            user: user.into(),
            auth: SshAuth::KeyFile { path: key.clone() },
        },
        initial_path: None,
        favorites: vec![],
    };
    let opts = |trust: Option<String>| ConnectOptions {
        secret: None,
        trust_fingerprint: trust,
        known_hosts: Some(known_hosts.clone()),
        known_certs: None,
    };

    // First contact: the host is unknown and the fingerprint is reported.
    let fp = match connections::connect(&conn, None, opts(None)).await {
        Err(Error::UnknownHost { fingerprint, .. }) => fingerprint,
        Err(e) => panic!("expected an unknown host, got {e}"),
        Ok(_) => panic!("expected an unknown host"),
    };
    assert!(fp.starts_with("SHA256:"));
    // Trusting a different fingerprint does not help.
    assert!(matches!(
        connections::connect(&conn, None, opts(Some("SHA256:nope".into()))).await,
        Err(Error::UnknownHost { .. })
    ));
    // Trusting the right one records it; later connections need no trust.
    connections::connect(&conn, None, opts(Some(fp.clone())))
        .await
        .unwrap();
    assert!(std::fs::read_to_string(&known_hosts)
        .unwrap()
        .contains(host));
    let c = connections::connect(&conn, None, opts(None)).await.unwrap();

    // A wrong password is a sign-in failure, not a crash.
    let pw = Connection {
        endpoint: Endpoint::Sftp {
            host: host.into(),
            port: port.parse().unwrap(),
            user: user.into(),
            auth: SshAuth::Password,
        },
        ..conn.clone()
    };
    assert!(matches!(
        connections::connect(&pw, Some("wrong".into()), opts(None)).await,
        Err(Error::AuthFailed(_))
    ));

    // With DUALPAL_TEST_SFTP_PASSWORD, the right password signs in too.
    if let Some(good) = dualpal_testkit::remote_env("DUALPAL_TEST_SFTP_PASSWORD") {
        connections::connect(&pw, Some(good), opts(None))
            .await
            .unwrap();
    }

    let base = c.fs.join(
        &c.start_path,
        &format!("dualpal-test-{}", uuid::Uuid::new_v4()),
    );
    c.fs.create_dir(&base).await.unwrap();
    // Permissions and times are kept over SFTP.
    put(&*c.fs, &c.fs.join(&base, "x.sh"), "#!/bin/sh").await;
    c.fs.set_mode(&c.fs.join(&base, "x.sh"), 0o750)
        .await
        .unwrap();
    c.fs.set_modified(&c.fs.join(&base, "x.sh"), 1_600_000_000_000)
        .await
        .unwrap();
    let st = c.fs.stat(&c.fs.join(&base, "x.sh")).await.unwrap();
    assert_eq!(st.mode, Some(0o750));
    assert_eq!(st.modified_ms, Some(1_600_000_000_000));
    c.fs.remove_file(&c.fs.join(&base, "x.sh")).await.unwrap();

    // With DUALPAL_TEST_SFTP_DROP (a command that cuts open SSH sessions,
    // e.g. `docker exec dualpal-test-sftp drop-connections`), an upload that
    // loses its connection midway carries on and arrives intact.
    if let Some(drop_cmd) = dualpal_testkit::remote_env("DUALPAL_TEST_SFTP_DROP") {
        upload_survives_a_dropped_connection(c.fs.clone(), &base, &drop_cmd).await;
    }

    sync_round_trip(c.fs.clone(), &base).await;
    exercise(c.fs.clone(), &base).await;
    c.fs.remove_dir(&base).await.unwrap();
}

async fn upload_survives_a_dropped_connection(fs: Arc<dyn FileSystem>, base: &str, drop_cmd: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let len = 96 * 1024 * 1024;
    let data: Vec<u8> = (0..len).map(|i| (i * 7 + i / 251) as u8).collect();
    let src = tmp.path().join("big.bin");
    std::fs::write(&src, &data).unwrap();

    let registry = Arc::new(Registry::with_local());
    registry.register(fs.clone());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(10)), 1);
    let id = q.enqueue(Operation::Copy {
        sources: vec![Location::new("local", src.to_string_lossy())],
        destination: Location::new(fs.id(), base),
        conflict: ConflictPolicy::Fail,
    });
    // Cut the connection once some of the file is across.
    let mut dropped = false;
    while !q.get(id).unwrap().status.is_finished() {
        if !dropped && q.get(id).unwrap().progress.bytes_done > 16 * 1024 * 1024 {
            let out = std::process::Command::new("sh")
                .args(["-c", drop_cmd])
                .output()
                .unwrap();
            assert!(out.status.success(), "{drop_cmd}: {out:?}");
            dropped = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let info = q.wait(id).await.unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(dropped, "the upload finished before the connection was cut");

    let remote = fs.join(base, "big.bin");
    let mut got = Vec::new();
    fs.open_read(&remote)
        .await
        .unwrap()
        .read_to_end(&mut got)
        .await
        .unwrap();
    assert!(got == data, "the uploaded file differs");
    let leftovers: Vec<String> = names(&*fs, base)
        .await
        .into_iter()
        .filter(|n| n.contains("dualpal"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    fs.remove_file(&remote).await.unwrap();
}

#[tokio::test]
/// EDIT-001
async fn remote_edits_upload_on_save() {
    use dualpal_core::remote_edit::{EditStatus, RemoteEditor};
    let op = opendal::Operator::new(opendal::services::Memory::default())
        .unwrap()
        .finish();
    let fs: Arc<dyn FileSystem> =
        Arc::new(S3Fs::from_operator("conn:mem".into(), "Memory".into(), op));
    put(&*fs, "/notes.md", "v1").await;
    let registry = Arc::new(Registry::with_local());
    registry.register(fs.clone());
    let tmp = tempfile::tempdir().unwrap();
    let editor = RemoteEditor::new(registry, tmp.path());
    let mut events = editor.subscribe();

    let s = editor
        .start(Location::new("conn:mem", "/notes.md"))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&s.local_path).unwrap(), "v1");
    // Starting again reuses the session.
    assert_eq!(
        editor
            .start(Location::new("conn:mem", "/notes.md"))
            .await
            .unwrap()
            .id,
        s.id
    );

    // "Save" in the editor: a different size guarantees a new signature.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    std::fs::write(&s.local_path, "version 2").unwrap();
    let uploaded = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let e = events.recv().await.unwrap();
            if e.status == EditStatus::Uploaded {
                return e;
            }
        }
    })
    .await
    .expect("upload within 10 s");
    assert_eq!(uploaded.uploads, 1);
    assert_eq!(get(&*fs, "/notes.md").await, "version 2");

    assert!(editor.stop(s.id));
    assert!(!Path::new(&s.local_path).exists());
    assert!(editor.list().is_empty());
}

/// A real S3-compatible server (MinIO, moto…):
/// `DUALPAL_TEST_S3=http://127.0.0.1:9000|bucket|access-key|secret-key`.
/// The bucket must exist; the test works in a fresh folder inside it.
#[tokio::test]
async fn s3_backend_against_a_real_server() {
    let Some(spec) = dualpal_testkit::remote_env("DUALPAL_TEST_S3") else {
        return;
    };
    let parts: Vec<&str> = spec.split('|').collect();
    let [endpoint, bucket, key, secret] = parts[..] else {
        panic!("DUALPAL_TEST_S3 must be endpoint|bucket|key|secret");
    };
    let conn = Connection {
        id: "s3t".into(),
        name: "Test bucket".into(),
        endpoint: Endpoint::S3 {
            bucket: bucket.into(),
            region: "us-east-1".into(),
            endpoint: Some(endpoint.into()),
            access_key_id: key.into(),
            path_style: true,
        },
        initial_path: None,
        favorites: vec![],
    };

    // A wrong secret is reported as a sign-in failure (emulators such as
    // moto accept any key, so only check servers that refuse it).
    match connections::connect(
        &conn,
        Some("wrong-secret".into()),
        ConnectOptions::default(),
    )
    .await
    {
        Err(e) => assert!(matches!(e, Error::AuthFailed(_)), "{e}"),
        Ok(_) => eprintln!("note: this server accepts any secret key"),
    }

    let c = connections::connect(&conn, Some(secret.into()), ConnectOptions::default())
        .await
        .unwrap();
    assert_eq!(c.start_path, "/");
    let base = format!("/dualpal-test-{}", uuid::Uuid::new_v4());
    c.fs.create_dir(&base).await.unwrap();
    // A large file goes up as a multipart upload (parts of 8 MiB).
    let big = c.fs.join(&base, "big.bin");
    {
        let mut w = c.fs.open_write(&big).await.unwrap();
        let chunk = vec![42u8; 1 << 20];
        for _ in 0..20 {
            w.write_all(&chunk).await.unwrap();
        }
        w.shutdown().await.unwrap();
    }
    assert_eq!(c.fs.stat(&big).await.unwrap().size, 20 << 20);
    let mut r = c.fs.open_read(&big).await.unwrap();
    let mut n = 0usize;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let k = r.read(&mut buf).await.unwrap();
        if k == 0 {
            break;
        }
        assert!(buf[..k].iter().all(|b| *b == 42));
        n += k;
    }
    assert_eq!(n, 20 << 20);
    c.fs.remove_file(&big).await.unwrap();

    sync_round_trip(c.fs.clone(), &base).await;
    exercise(c.fs.clone(), &base).await;
    c.fs.remove_dir(&base).await.unwrap();
    assert!(matches!(c.fs.stat(&base).await, Err(Error::NotFound(_))));
}

#[tokio::test]
/// EDIT-002
async fn unsent_edits_survive_failures_and_restarts() {
    use dualpal_core::remote_edit::{EditStatus, RemoteEditor};
    let op = opendal::Operator::new(opendal::services::Memory::default())
        .unwrap()
        .finish();
    let fs: Arc<dyn FileSystem> =
        Arc::new(S3Fs::from_operator("conn:mem".into(), "Memory".into(), op));
    put(&*fs, "/a.txt", "v1").await;
    let registry = Arc::new(Registry::with_local());
    registry.register(fs.clone());
    let tmp = tempfile::tempdir().unwrap();
    let editor = RemoteEditor::new(registry.clone(), tmp.path());
    let mut events = editor.subscribe();
    let s = editor
        .start(Location::new("conn:mem", "/a.txt"))
        .await
        .unwrap();

    // The editor saves, and the server is disconnected before the upload.
    std::fs::write(&s.local_path, "unsent v2").unwrap();
    editor.stop_all_on("conn:mem");
    registry.unregister("conn:mem");
    let failed = editor.list().into_iter().find(|x| x.id == s.id).unwrap();
    assert_eq!(failed.status, EditStatus::Failed, "kept, not deleted");
    assert!(Path::new(&s.local_path).exists());
    while events.try_recv().is_ok() {}

    // "Restart": a new editor over the same folder finds the unsent save.
    drop(editor);
    let editor = RemoteEditor::new(registry.clone(), tmp.path());
    let recovered = editor.list();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, EditStatus::Failed);
    assert_eq!(
        std::fs::read_to_string(&recovered[0].local_path).unwrap(),
        "unsent v2"
    );

    // Reconnected: uploading sends it and watching resumes.
    registry.register(fs.clone());
    let up = editor.upload_now(recovered[0].id).await.unwrap();
    assert_eq!(up.status, EditStatus::Uploaded);
    assert_eq!(get(&*fs, "/a.txt").await, "unsent v2");

    // A copy whose changes all reached the server is cleaned up on restart.
    drop(editor);
    let editor = RemoteEditor::new(registry.clone(), tmp.path());
    assert!(editor.list().is_empty());
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
/// VFS-PATH-001
fn server_names_cannot_escape_local_folders() {
    use dualpal_core::vfs::{is_safe_local_name, safe_local_name};
    assert!(is_safe_local_name("notes.txt"));
    assert!(!is_safe_local_name(".."));
    assert!(!is_safe_local_name("a/b"));
    assert!(!is_safe_local_name(""));
    #[cfg(windows)]
    {
        assert!(!is_safe_local_name("C:\\Users\\x.txt"));
        assert!(!is_safe_local_name("..\\x"));
    }
    assert_eq!(safe_local_name("C:\\Users\\x.txt"), "C__Users_x.txt");
    assert_eq!(safe_local_name(".."), "file");
    assert_eq!(safe_local_name("a/../b"), "a_.._b");
}

/// `user:password@host:port` (an empty user signs in anonymously).
fn ftp_spec(var: &str) -> Option<(String, String, String, u16)> {
    let spec = dualpal_testkit::remote_env(var)?;
    let (cred, addr) = spec.rsplit_once('@').expect("user:password@host:port");
    let (user, pass) = cred.split_once(':').unwrap_or((cred, ""));
    let (host, port) = addr.rsplit_once(':').expect("host:port");
    Some((user.into(), pass.into(), host.into(), port.parse().unwrap()))
}

async fn ftp_round_trip(var: &str, security: dualpal_core::vfs::FtpSecurity) {
    let Some((user, pass, host, port)) = ftp_spec(var) else {
        eprintln!("skipped: {var} is not set");
        return;
    };
    let conn = Connection {
        id: format!("ftp-{security:?}"),
        name: "Test FTP".into(),
        endpoint: Endpoint::Ftp {
            host,
            port,
            user,
            security,
        },
        initial_path: None,
        favorites: vec![],
    };
    let tmp = tempfile::tempdir().unwrap();
    let known = tmp.path().join("known_certificates.json");
    let opts = |trust: Option<String>| ConnectOptions {
        trust_fingerprint: trust,
        known_certs: Some(known.clone()),
        ..Default::default()
    };

    // A wrong password is a sign-in failure.
    if !pass.is_empty() {
        let wrong = connections::connect(&conn, Some("wrong".into()), opts(None)).await;
        match wrong {
            Err(Error::AuthFailed(_)) => {}
            // Self-signed FTPS servers are asked about before the password.
            Err(Error::UnknownHost { .. }) => {}
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("a wrong password was accepted"),
        }
    }
    let secret = Some(pass.clone());
    let c = match connections::connect(&conn, secret.clone(), opts(None)).await {
        Ok(c) => c,
        // A self-signed certificate: confirm it, then it is remembered.
        Err(Error::UnknownHost { fingerprint, .. }) => {
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
            assert!(matches!(
                connections::connect(&conn, secret.clone(), opts(Some("SHA256:00".into()))).await,
                Err(Error::UnknownHost { .. })
            ));
            connections::connect(&conn, secret.clone(), opts(Some(fingerprint)))
                .await
                .unwrap();
            connections::connect(&conn, secret.clone(), opts(None))
                .await
                .unwrap()
        }
        Err(e) => panic!("{e}"),
    };
    let base = c.fs.join(
        &c.start_path,
        &format!("dualpal-test-{}", uuid::Uuid::new_v4()),
    );
    c.fs.create_dir(&base).await.unwrap();
    // A file large enough to take many reads, and reading from an offset.
    let big = c.fs.join(&base, "big.bin");
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    {
        let mut w = c.fs.open_write(&big).await.unwrap();
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
    }
    assert_eq!(c.fs.stat(&big).await.unwrap().size, data.len() as u64);
    let mut r = c.fs.open_read_at(&big, 1_000_000).await.unwrap();
    let mut back = Vec::new();
    r.read_to_end(&mut back).await.unwrap();
    assert_eq!(back, data[1_000_000..]);
    drop(r);
    // Finish a cut-off upload from where it stopped.
    let cut = c.fs.join(&base, "cut.bin");
    {
        let mut w = c.fs.open_write(&cut).await.unwrap();
        w.write_all(&data[..1_200_000]).await.unwrap();
        w.shutdown().await.unwrap();
        assert!(c.fs.resumable_writes());
        let mut w = c.fs.open_write_at(&cut, 1_000_000).await.unwrap();
        w.write_all(&data[1_000_000..]).await.unwrap();
        w.shutdown().await.unwrap();
    }
    let mut whole = Vec::new();
    c.fs.open_read(&cut)
        .await
        .unwrap()
        .read_to_end(&mut whole)
        .await
        .unwrap();
    assert!(whole == data, "the resumed upload differs");
    c.fs.remove_file(&cut).await.unwrap();
    c.fs.remove_file(&big).await.unwrap();

    sync_round_trip(c.fs.clone(), &base).await;
    exercise(c.fs.clone(), &base).await;
    c.fs.remove_dir(&base).await.unwrap();
    assert!(matches!(c.fs.stat(&base).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn ftp_backend_against_a_real_server() {
    ftp_round_trip("DUALPAL_TEST_FTP", dualpal_core::vfs::FtpSecurity::None).await;
}

#[tokio::test]
async fn ftps_backend_against_a_real_server() {
    ftp_round_trip(
        "DUALPAL_TEST_FTPS",
        dualpal_core::vfs::FtpSecurity::Explicit,
    )
    .await;
}

#[tokio::test]
/// CON-019
async fn ftps_implicit_backend_against_a_real_server() {
    ftp_round_trip(
        "DUALPAL_TEST_FTPS_IMPLICIT",
        dualpal_core::vfs::FtpSecurity::Implicit,
    )
    .await;
}

#[tokio::test]
async fn webdav_backend_against_a_real_server() {
    // `user:password@http://host:port/path`
    let Some(spec) = dualpal_testkit::remote_env("DUALPAL_TEST_WEBDAV") else {
        return;
    };
    let (cred, url) = spec.split_once('@').expect("user:password@url");
    let (user, pass) = cred.split_once(':').unwrap_or((cred, ""));
    let conn = Connection {
        id: "dav".into(),
        name: "Test WebDAV".into(),
        endpoint: Endpoint::WebDav {
            url: url.into(),
            user: user.into(),
        },
        initial_path: None,
        favorites: vec![],
    };
    if !pass.is_empty() {
        match connections::connect(&conn, Some("wrong".into()), ConnectOptions::default()).await {
            Err(Error::AuthFailed(_)) => {}
            Err(e) => panic!("expected a sign-in failure, got {e}"),
            Ok(_) => panic!("a wrong password was accepted"),
        }
    }
    let c = connections::connect(&conn, Some(pass.into()), ConnectOptions::default())
        .await
        .unwrap();
    let base = format!("/dualpal-test-{}", uuid::Uuid::new_v4());
    c.fs.create_dir(&base).await.unwrap();
    // Uploads stream, so a large file and odd names work.
    let big = c.fs.join(&base, "big file #1 (ä).bin");
    {
        let mut w = c.fs.open_write(&big).await.unwrap();
        let chunk = vec![42u8; 1 << 20];
        for _ in 0..20 {
            w.write_all(&chunk).await.unwrap();
        }
        w.shutdown().await.unwrap();
    }
    assert_eq!(c.fs.stat(&big).await.unwrap().size, 20 << 20);
    assert_eq!(names(&*c.fs, &base).await, ["big file #1 (ä).bin"]);
    c.fs.remove_file(&big).await.unwrap();
    sync_round_trip(c.fs.clone(), &base).await;
    exercise(c.fs.clone(), &base).await;
    c.fs.remove_dir(&base).await.unwrap();
    assert!(matches!(c.fs.stat(&base).await, Err(Error::NotFound(_))));
}
