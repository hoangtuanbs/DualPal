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
    exercise(fs, "/work").await;
}

#[test]
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

/// `DUALPAL_TEST_SFTP=user@host:port`, `DUALPAL_TEST_SFTP_KEY=/path/to/key`.
#[tokio::test]
async fn sftp_backend_against_a_real_server() {
    let Ok(target) = std::env::var("DUALPAL_TEST_SFTP") else {
        eprintln!("skipped: DUALPAL_TEST_SFTP is not set");
        return;
    };
    let key = std::env::var("DUALPAL_TEST_SFTP_KEY").expect("DUALPAL_TEST_SFTP_KEY");
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

    exercise(c.fs.clone(), &base).await;
    c.fs.remove_dir(&base).await.unwrap();
}

#[tokio::test]
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
