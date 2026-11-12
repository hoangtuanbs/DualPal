//! Import cases for WinSCP, FileZilla and Cyberduck exports (IMP-*).
//! All hosts and users are fake.

use dualpal_core::connections::{Endpoint, SshAuth};
use dualpal_core::import::{
    parse_cyberduck_bookmarks, parse_filezilla_sitemanager, parse_winscp_ini,
};
use dualpal_core::vfs::FtpSecurity;

/// WinSCP's password obfuscation, the inverse of the decoder: `0xFF` flag,
/// a skip byte, the length, then `user + host + password`.
fn winscp_encode(password: &str, user: &str, host: &str) -> String {
    let plain = format!("{user}{host}{password}");
    let mut bytes = vec![0xFF, 0x00, plain.len() as u8];
    bytes.extend(plain.bytes());
    bytes
        .iter()
        .map(|b| format!("{:02X}", (!b) ^ 0xA3))
        .collect()
}

// ---- WinSCP -------------------------------------------------------------------

const WINSCP: &str = "\
[Sessions\\Default%20Settings]
HostName=
FSProtocol=1

[Sessions\\Work%20Server]
HostName=files.example.com
PortNumber=2222
UserName=alice
FSProtocol=1
Password=PASSWORD

[Sessions\\Team\\FTP%20Mirror]
HostName=ftp.example.com
UserName=bob
FSProtocol=5
FtpSecure=2
Password=

[Sessions\\Old%20Box]
HostName=old.example.com
FSProtocol=1
UserName=carol
Hidden=1

[Sessions\\Scp%20Only]
HostName=scp.example.com
UserName=dave
FSProtocol=0

[Sessions\\Dav]
HostName=dav.example.com
UserName=erin
FSProtocol=6

[Sessions\\No%20Host]
UserName=frank
FSProtocol=1

[Sessions\\Key%20Login]
HostName=keys.example.com
UserName=grace
FSProtocol=2
PublicKeyFile=C:\\Keys\\grace.ppk

[Configuration\\Interface]
Language=1033
";

#[test]
fn imp_001_winscp_imports_sftp_and_ftp_with_decoded_password() {
    let encoded = winscp_encode("s3cret", "alice", "files.example.com");
    let text = WINSCP.replace("Password=PASSWORD", &format!("Password={encoded}"));
    let report = parse_winscp_ini(&text);

    let names: Vec<&str> = report
        .connections
        .iter()
        .map(|c| c.connection.name.as_str())
        .collect();
    assert_eq!(names, vec!["Work Server", "Team/FTP Mirror", "Key Login"]);

    let work = &report.connections[0];
    assert_eq!(
        work.connection.endpoint,
        Endpoint::Sftp {
            host: "files.example.com".into(),
            port: 2222,
            user: "alice".into(),
            auth: SshAuth::Password,
        }
    );
    assert_eq!(work.secret.as_deref(), Some("s3cret"));
    assert!(!work.password_missing);
    assert!(work.connection.id.is_empty());

    let ftp = &report.connections[1];
    assert_eq!(
        ftp.connection.endpoint,
        Endpoint::Ftp {
            host: "ftp.example.com".into(),
            port: 21,
            user: "bob".into(),
            security: FtpSecurity::Explicit,
        }
    );
    assert_eq!(ftp.secret, None);
    assert!(ftp.password_missing);
}

#[test]
fn imp_002_winscp_skips_defaults_hidden_and_unsupported_with_reasons() {
    let report = parse_winscp_ini(WINSCP);
    let skipped: Vec<(&str, &str)> = report
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason.as_str()))
        .collect();

    // Default Settings, the hidden session and configuration are not listed at all.
    assert!(!skipped.iter().any(|(n, _)| n.contains("Default")));
    assert!(!skipped.iter().any(|(n, _)| *n == "Old Box"));
    assert!(!report
        .connections
        .iter()
        .any(|c| c.connection.name == "Old Box"));

    assert!(skipped.contains(&("Scp Only", "SCP sessions are not supported")));
    assert!(skipped.contains(&("Dav", "WebDAV sessions are not imported from WinSCP yet")));
    assert!(skipped.contains(&("No Host", "the session has no host name")));
}

#[test]
fn imp_003_winscp_key_file_auth_has_no_password_requirement() {
    let report = parse_winscp_ini(WINSCP);
    let key = report
        .connections
        .iter()
        .find(|c| c.connection.name == "Key Login")
        .expect("key session imported");
    assert_eq!(
        key.connection.endpoint,
        Endpoint::Sftp {
            host: "keys.example.com".into(),
            port: 22,
            user: "grace".into(),
            auth: SshAuth::KeyFile {
                path: "C:\\Keys\\grace.ppk".into()
            },
        }
    );
    assert!(key.password_missing);
}

#[test]
fn imp_004_winscp_wrong_password_key_is_not_imported() {
    // Encoded for another user: the decoded text does not start with user+host.
    let encoded = winscp_encode("s3cret", "mallory", "files.example.com");
    let text = format!("[Sessions\\X]\nHostName=files.example.com\nUserName=alice\nFSProtocol=1\nPassword={encoded}\n");
    let report = parse_winscp_ini(&text);
    assert_eq!(report.connections.len(), 1);
    assert_eq!(report.connections[0].secret, None);
    assert!(report.connections[0].password_missing);
}

#[test]
fn imp_005_winscp_ftp_encryption_mapping() {
    let text = "\
[Sessions\\Implicit]
HostName=a.example.com
UserName=alice
FSProtocol=5
FtpSecure=1

[Sessions\\Plain]
HostName=b.example.com
UserName=alice
FSProtocol=5
FtpSecure=0
";
    let report = parse_winscp_ini(text);
    assert_eq!(
        report.connections[0].connection.endpoint,
        Endpoint::Ftp {
            host: "a.example.com".into(),
            port: 990,
            user: "alice".into(),
            security: FtpSecurity::Implicit,
        }
    );
    assert_eq!(
        report.connections[1].connection.endpoint,
        Endpoint::Ftp {
            host: "b.example.com".into(),
            port: 21,
            user: "alice".into(),
            security: FtpSecurity::None,
        }
    );
}

// ---- FileZilla ----------------------------------------------------------------

const FILEZILLA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<FileZilla3>
  <Servers>
    <Server>
      <Host>files.example.com</Host>
      <Port>22</Port>
      <Protocol>1</Protocol>
      <Logontype>1</Logontype>
      <User>alice</User>
      <Pass encoding="base64">czNjcmV0</Pass>
      <Name>Files &amp; Docs</Name>
    </Server>
    <Folder expanded="1">Clients
      <Server>
        <Host>sftp.example.org</Host>
        <Port>2200</Port>
        <Protocol>1</Protocol>
        <Logontype>1</Logontype>
        <User>bob</User>
        <Keyfile>/home/bob/.ssh/id_ed25519</Keyfile>
        <Name>Client Box</Name>
      </Server>
      <Server>
        <Host>ftp.example.net</Host>
        <Protocol>0</Protocol>
        <User>carol</User>
        <Name>Legacy FTP</Name>
      </Server>
      <Server>
        <Protocol>1</Protocol>
        <User>dave</User>
        <Name>No Host</Name>
      </Server>
    </Folder>
    <Server>
      <Host>s3.example.com</Host>
      <Protocol>3</Protocol>
      <User>AKIAEXAMPLE</User>
      <Name>Bucket Thing</Name>
    </Server>
  </Servers>
</FileZilla3>
"#;

#[test]
fn imp_010_filezilla_imports_sftp_with_base64_password_and_folders() {
    let report = parse_filezilla_sitemanager(FILEZILLA);

    let names: Vec<&str> = report
        .connections
        .iter()
        .map(|c| c.connection.name.as_str())
        .collect();
    assert_eq!(names, vec!["Files & Docs", "Clients/Client Box"]);

    let docs = &report.connections[0];
    assert_eq!(
        docs.connection.endpoint,
        Endpoint::Sftp {
            host: "files.example.com".into(),
            port: 22,
            user: "alice".into(),
            auth: SshAuth::Password,
        }
    );
    assert_eq!(docs.secret.as_deref(), Some("s3cret"));
    assert!(!docs.password_missing);

    let client = &report.connections[1];
    assert_eq!(
        client.connection.endpoint,
        Endpoint::Sftp {
            host: "sftp.example.org".into(),
            port: 2200,
            user: "bob".into(),
            auth: SshAuth::KeyFile {
                path: "/home/bob/.ssh/id_ed25519".into()
            },
        }
    );
    assert!(client.password_missing);
}

#[test]
fn imp_011_filezilla_skips_ftp_ambiguity_missing_host_and_other_protocols() {
    let report = parse_filezilla_sitemanager(FILEZILLA);
    let reasons: Vec<(&str, &str)> = report
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason.as_str()))
        .collect();
    assert!(reasons
        .iter()
        .any(|(n, r)| *n == "Clients/Legacy FTP" && r.starts_with("FTP entries are not imported")));
    assert!(reasons.contains(&("Clients/No Host", "the server has no host name")));
    assert!(reasons.contains(&("Bucket Thing", "protocol 3 is not supported")));
}

#[test]
fn imp_012_filezilla_rejects_other_documents() {
    let report = parse_filezilla_sitemanager("<Other><Thing/></Other>");
    assert!(report.connections.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].reason.contains("not a FileZilla"));

    let broken = parse_filezilla_sitemanager("<FileZilla3><Servers>");
    assert!(broken.connections.is_empty());
    assert!(broken.skipped[0].reason.contains("not readable as XML"));
}

// ---- Cyberduck ----------------------------------------------------------------

const CYBERDUCK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>Hostname</key>
    <string>files.example.com</string>
    <key>Nickname</key>
    <string>Files &amp; More</string>
    <key>Port</key>
    <integer>22</integer>
    <key>Protocol</key>
    <string>sftp</string>
    <key>Username</key>
    <string>alice</string>
  </dict>
  <dict>
    <key>Hostname</key>
    <string>dav.example.com</string>
    <key>Protocol</key>
    <string>dav-ssl</string>
    <key>Username</key>
    <string>bob</string>
    <key>Path</key>
    <string>/remote.php/dav</string>
  </dict>
  <dict>
    <key>Hostname</key>
    <string>ftp.example.com</string>
    <key>Nickname</key>
    <string>Old FTP</string>
    <key>Protocol</key>
    <string>ftp</string>
    <key>Username</key>
    <string></string>
  </dict>
  <dict>
    <key>Hostname</key>
    <string>s3.example.com</string>
    <key>Protocol</key>
    <string>s3</string>
    <key>Username</key>
    <string>AKIAEXAMPLE</string>
  </dict>
  <dict>
    <key>Nickname</key>
    <string>No Host</string>
    <key>Protocol</key>
    <string>sftp</string>
    <key>Username</key>
    <string>carol</string>
  </dict>
</array>
</plist>
"#;

#[test]
fn imp_020_cyberduck_imports_sftp_webdav_and_ftp_without_passwords() {
    let report = parse_cyberduck_bookmarks(CYBERDUCK);

    let names: Vec<&str> = report
        .connections
        .iter()
        .map(|c| c.connection.name.as_str())
        .collect();
    assert_eq!(names, vec!["Files & More", "dav.example.com", "Old FTP"]);

    assert_eq!(
        report.connections[0].connection.endpoint,
        Endpoint::Sftp {
            host: "files.example.com".into(),
            port: 22,
            user: "alice".into(),
            auth: SshAuth::Password,
        }
    );
    assert_eq!(
        report.connections[1].connection.endpoint,
        Endpoint::WebDav {
            url: "https://dav.example.com/remote.php/dav".into(),
            user: "bob".into(),
        }
    );
    assert_eq!(
        report.connections[2].connection.endpoint,
        Endpoint::Ftp {
            host: "ftp.example.com".into(),
            port: 21,
            user: String::new(),
            security: FtpSecurity::None,
        }
    );
    // Cyberduck keeps passwords in the keychain, so none come across.
    assert!(report.connections.iter().all(|c| c.secret.is_none()));
    assert!(report.connections[0].password_missing);
    assert!(
        !report.connections[2].password_missing,
        "anonymous FTP needs no password"
    );
}

#[test]
fn imp_021_cyberduck_skips_s3_and_entries_without_host() {
    let report = parse_cyberduck_bookmarks(CYBERDUCK);
    let reasons: Vec<(&str, &str)> = report
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason.as_str()))
        .collect();
    assert!(reasons.contains(&(
        "s3.example.com",
        "S3 bookmarks are not imported from Cyberduck yet"
    )));
    assert!(reasons.contains(&("No Host", "the bookmark has no host name")));
}

#[test]
fn imp_022_cyberduck_rejects_other_documents() {
    let report = parse_cyberduck_bookmarks("<html><body>hi</body></html>");
    assert!(report.connections.is_empty());
    assert!(report.skipped[0]
        .reason
        .contains("not a Cyberduck bookmark file"));
}

#[test]
fn imp_023_unreadable_text_never_panics() {
    for text in ["", "not a file", "[Sessions\\", "<<>>", "&#xZZ;"] {
        let _ = parse_winscp_ini(text);
        let _ = parse_filezilla_sitemanager(text);
        let _ = parse_cyberduck_bookmarks(text);
    }
}
