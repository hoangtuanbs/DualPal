//! Connection, secret and sign-in cases that do not need a server (Phase 1).

use std::sync::Arc;

use dualpal_core::connections::{
    Connection, Connections, Endpoint, MemorySecrets, RemoteFavorite, SecretStore, SshAuth,
};
use dualpal_testkit::FailingSecrets;

fn sftp(id: &str, auth: SshAuth) -> Connection {
    Connection {
        id: id.into(),
        name: format!("conn {id}"),
        endpoint: Endpoint::Sftp {
            host: "example.com".into(),
            port: 22,
            user: "me".into(),
            auth,
        },
        initial_path: None,
        favorites: vec![],
    }
}

/// CON-002 — save, list, get, remove and reorder all persist.
#[test]
fn connections_crud_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());

    let a = sftp("a", SshAuth::Password);
    let b = sftp("b", SshAuth::Agent);
    Connections::open(&path, secrets.clone())
        .save(a.clone(), None)
        .unwrap();
    Connections::open(&path, secrets.clone())
        .save(b.clone(), None)
        .unwrap();

    let c = Connections::open(&path, secrets.clone());
    assert_eq!(c.list().len(), 2);
    assert_eq!(c.get("a").unwrap().name, "conn a");

    c.reorder(&["b".into(), "a".into()]).unwrap();
    let reopened = Connections::open(&path, secrets.clone());
    assert_eq!(reopened.list()[0].id, "b");

    reopened.remove("b").unwrap();
    assert_eq!(reopened.list().len(), 1);
    assert!(reopened.get("b").is_err());
}

/// CON-003 — removing a connection also removes its secret.
#[test]
fn removing_a_connection_removes_its_secret() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());
    let c = Connections::in_memory(secrets.clone());
    c.save(sftp("a", SshAuth::Password), Some("S3CR3T".into()))
        .unwrap();
    assert_eq!(c.secret("a").unwrap().as_deref(), Some("S3CR3T"));
    c.remove("a").unwrap();
    assert_eq!(c.secret("a").unwrap(), None, "the secret must be gone");
}

/// CON-004 — server favorites can be added and removed.
#[test]
fn server_favorites_add_and_remove() {
    let c = Connections::in_memory(Arc::new(MemorySecrets::default()));
    c.save(sftp("a", SshAuth::Agent), None).unwrap();
    c.add_favorite(
        "a",
        RemoteFavorite {
            name: "Docs".into(),
            path: "/home/me/docs".into(),
        },
    )
    .unwrap();
    assert_eq!(c.get("a").unwrap().favorites.len(), 1);

    c.remove_favorite("a", "/home/me/docs").unwrap();
    assert!(c.get("a").unwrap().favorites.is_empty());
}

/// CON-005 — the file-system id is stable and unique.
#[test]
fn fs_id_is_stable_and_unique() {
    let a = sftp("abc", SshAuth::Agent);
    let b = sftp("xyz", SshAuth::Agent);
    assert_eq!(a.fs_id(), "conn:abc");
    assert_eq!(a.fs_id(), a.fs_id());
    assert_ne!(a.fs_id(), b.fs_id());
}

/// CON-006 — which auth methods need a secret.
#[test]
fn uses_secret_per_auth_method() {
    assert!(sftp("p", SshAuth::Password).uses_secret());
    assert!(!sftp("k", SshAuth::Agent).uses_secret());
    // A key file may carry a passphrase, so the app conservatively asks for
    // a secret even though an unencrypted key needs none.
    assert!(sftp("k", SshAuth::KeyFile { path: "/k".into() }).uses_secret());
    assert!(Connection {
        id: "s3".into(),
        name: "s3".into(),
        endpoint: Endpoint::S3 {
            bucket: "b".into(),
            region: "r".into(),
            endpoint: None,
            access_key_id: "ak".into(),
            path_style: true,
        },
        initial_path: None,
        favorites: vec![],
    }
    .uses_secret());
}

/// CON-007 — each endpoint serialises to the JSON shape the UI expects.
#[test]
fn endpoint_json_shape() {
    let ftp = Connection {
        id: "f".into(),
        name: "f".into(),
        endpoint: Endpoint::Ftp {
            host: "h".into(),
            port: 21,
            user: "u".into(),
            security: dualpal_core::vfs::FtpSecurity::Implicit,
        },
        initial_path: None,
        favorites: vec![],
    };
    insta::assert_json_snapshot!(ftp.endpoint, @r###"
    {
      "type": "ftp",
      "host": "h",
      "port": 21,
      "user": "u",
      "security": "implicit"
    }
    "###);

    let s3 = Endpoint::S3 {
        bucket: "b".into(),
        region: "us-east-1".into(),
        endpoint: Some("http://127.0.0.1:9000".into()),
        access_key_id: "ak".into(),
        path_style: true,
    };
    insta::assert_json_snapshot!(s3, @r###"
    {
      "type": "s3",
      "bucket": "b",
      "region": "us-east-1",
      "endpoint": "http://127.0.0.1:9000",
      "accessKeyId": "ak",
      "pathStyle": true
    }
    "###);
}

/// CON-008 — a failing secret store surfaces the error, never plaintext.
#[test]
fn secret_store_failure_surfaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let c = Connections::open(&path, Arc::new(FailingSecrets));
    let err = c
        .save(
            sftp("a", SshAuth::Password),
            Some("plaintext-secret".into()),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid");
    // Nothing was written to disk with the secret in it.
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(!on_disk.contains("plaintext-secret"));
}
