//! Import saved servers from other apps: WinSCP, FileZilla and Cyberduck.
//!
//! These are pure parsers. They take the text of an export and return the
//! servers they can read, plus the reason each other entry was skipped. They
//! read no files and save nothing; the UI shows the report and saves the
//! selected servers through the normal connection path.
//!
//! Endpoints are never guessed. An entry without a host, or with a protocol
//! that cannot be mapped exactly, is skipped with a reason. A password is
//! returned only when the export holds one in a form this module can decode.
//! It is meant for the secret store, never for the connections file.

use std::collections::HashMap;

use serde::Serialize;

use crate::connections::{Connection, Endpoint, SshAuth};
use crate::vfs::FtpSecurity;

/// The result of reading one export.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Servers that can be saved as they are.
    pub connections: Vec<ImportedConnection>,
    /// Entries that were not imported, with the reason.
    pub skipped: Vec<SkippedEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedConnection {
    /// Ready to save. The id is empty, so the store assigns one.
    pub connection: Connection,
    /// The decoded password, when the export had one this module can read.
    /// Save it with the connection; it is never written to the connections file.
    pub secret: Option<String>,
    /// True when the connection needs a password (or key passphrase) and the
    /// export did not contain one.
    pub password_missing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedEntry {
    pub name: String,
    pub reason: String,
}

impl ImportedConnection {
    fn new(connection: Connection, secret: Option<String>) -> Self {
        let password_missing = connection.uses_secret() && secret.is_none();
        Self {
            connection,
            secret,
            password_missing,
        }
    }
}

impl ImportReport {
    fn add(&mut self, name: &str, draft: Result<(Connection, Option<String>), String>) {
        match draft {
            Ok((connection, secret)) => self
                .connections
                .push(ImportedConnection::new(connection, secret)),
            Err(reason) => self.skipped.push(SkippedEntry {
                name: name.to_string(),
                reason,
            }),
        }
    }

    /// A report for an export that could not be read at all.
    fn unreadable(reason: impl Into<String>) -> Self {
        Self {
            connections: Vec::new(),
            skipped: vec![SkippedEntry {
                name: "(file)".into(),
                reason: reason.into(),
            }],
        }
    }
}

/// A connection ready to save, or the reason it cannot be imported.
type Draft = Result<(Connection, Option<String>), String>;

fn connection(name: &str, host: &str, endpoint: Endpoint) -> Connection {
    let name = if name.trim().is_empty() {
        host.trim()
    } else {
        name.trim()
    };
    Connection {
        id: String::new(),
        name: name.to_string(),
        endpoint,
        initial_path: None,
        favorites: Vec::new(),
    }
}

/// `value` when it is set and not blank.
fn port(text: Option<&str>, default: u16) -> Result<u16, String> {
    match text {
        None => Ok(default),
        Some(t) => t
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("invalid port {t}")),
    }
}

// ---- WinSCP ------------------------------------------------------------------

/// Parse a WinSCP INI export (`[Sessions\NAME]` sections).
///
/// The "Default Settings" session and sessions marked `Hidden=1` are not
/// servers, so they are left out without a report entry.
pub fn parse_winscp_ini(text: &str) -> ImportReport {
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let name = header.trim_end().trim_end_matches(']');
            sections.push((name.to_string(), HashMap::new()));
        } else if let Some((key, value)) = line.split_once('=') {
            if let Some((_, values)) = sections.last_mut() {
                values.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }

    let mut report = ImportReport::default();
    for (section, values) in &sections {
        let Some(raw_name) = section.strip_prefix("Sessions\\") else {
            continue;
        };
        let name = percent_decode(raw_name);
        if name == "Default Settings" || field(values, "Hidden") == Some("1") {
            continue;
        }
        let display = name.replace('\\', "/");
        let draft = winscp_session(&display, values);
        report.add(&display, draft);
    }
    report
}

fn winscp_session(name: &str, s: &HashMap<String, String>) -> Draft {
    let host = field(s, "HostName").ok_or("the session has no host name")?;
    let user = field(s, "UserName").unwrap_or("");
    let secret = field(s, "Password").and_then(|p| winscp_decrypt(p, user, host));

    let endpoint = match field(s, "FSProtocol").unwrap_or("") {
        "1" | "2" => {
            if user.is_empty() {
                return Err("the session has no user name".into());
            }
            let auth = match field(s, "PublicKeyFile") {
                Some(path) => SshAuth::KeyFile { path: path.into() },
                None => SshAuth::Password,
            };
            Endpoint::Sftp {
                host: host.into(),
                port: port(field(s, "PortNumber"), 22)?,
                user: user.into(),
                auth,
            }
        }
        "5" => {
            let security = match field(s, "FtpSecure").unwrap_or("0") {
                "0" => FtpSecurity::None,
                "1" => FtpSecurity::Implicit,
                "2" | "3" => FtpSecurity::Explicit,
                other => return Err(format!("unknown FTP encryption setting {other}")),
            };
            let default_port = if matches!(security, FtpSecurity::Implicit) {
                990
            } else {
                21
            };
            Endpoint::Ftp {
                host: host.into(),
                port: port(field(s, "PortNumber"), default_port)?,
                user: user.into(),
                security,
            }
        }
        "0" => return Err("SCP sessions are not supported".into()),
        "6" => return Err("WebDAV sessions are not imported from WinSCP yet".into()),
        "7" => return Err("S3 sessions are not imported from WinSCP yet".into()),
        "" => return Err("the session does not name a protocol".into()),
        other => return Err(format!("protocol {other} is not supported")),
    };
    Ok((connection(name, host, endpoint), secret))
}

/// WinSCP's obfuscation of stored passwords: each character is stored as two
/// hex digits of `!(c ^ 0xA3)`. A leading `0xFF` flag means the plaintext was
/// prefixed with `user + host`, which is stripped here. Returns `None` when the
/// text does not decode to that prefix, so a wrong guess is never imported.
fn winscp_decrypt(obfuscated: &str, user: &str, host: &str) -> Option<String> {
    const MAGIC: u8 = 0xA3;
    const FLAG: u8 = 0xFF;
    let bytes = hex_bytes(obfuscated)?;
    let mut pos = 0;
    let mut next = || -> Option<u8> {
        let b = *bytes.get(pos)?;
        pos += 1;
        Some(!(b ^ MAGIC))
    };

    let first = next()?;
    let (length, prefixed) = if first == FLAG {
        next()?; // unused
        (next()? as usize, true)
    } else {
        (first as usize, false)
    };
    let mut chars = Vec::with_capacity(length);
    for _ in 0..length {
        chars.push(next()?);
    }
    let text = String::from_utf8(chars).ok()?;
    let password = if prefixed {
        text.strip_prefix(&format!("{user}{host}"))?.to_string()
    } else {
        text
    };
    (!password.is_empty()).then_some(password)
}

// ---- FileZilla ---------------------------------------------------------------

/// Parse FileZilla's `sitemanager.xml`. Servers inside folders are imported
/// with the folder path in their name.
pub fn parse_filezilla_sitemanager(xml: &str) -> ImportReport {
    let root = match parse_xml(xml) {
        Ok(root) => root,
        Err(e) => return ImportReport::unreadable(format!("not readable as XML: {e}")),
    };
    let Some(fz) = root.child("FileZilla3") else {
        return ImportReport::unreadable("not a FileZilla site manager file");
    };
    let mut report = ImportReport::default();
    if let Some(servers) = fz.child("Servers") {
        filezilla_walk(servers, "", &mut report);
    }
    report
}

fn filezilla_walk(node: &Node, folder: &str, report: &mut ImportReport) {
    for child in &node.children {
        match child.name.as_str() {
            "Folder" => {
                let name = child.text.trim();
                let path = if folder.is_empty() {
                    name.to_string()
                } else {
                    format!("{folder}/{name}")
                };
                filezilla_walk(child, &path, report);
            }
            "Server" => {
                let host = child.child_text("Host").unwrap_or_default();
                let name = child.child_text("Name").unwrap_or_default();
                let label = if folder.is_empty() {
                    name.clone()
                } else {
                    format!("{folder}/{name}")
                };
                let draft = filezilla_server(&label, &host, child);
                report.add(&label, draft);
            }
            _ => {}
        }
    }
}

fn filezilla_server(label: &str, host: &str, s: &Node) -> Draft {
    if host.is_empty() {
        return Err("the server has no host name".into());
    }
    let user = s.child_text("User").unwrap_or_default();
    let secret = s.child("Pass").and_then(filezilla_password);
    let endpoint = match s.child_text("Protocol").unwrap_or_default().as_str() {
        "1" => {
            if user.is_empty() {
                return Err("the server has no user name".into());
            }
            let auth = match s.child_text("Keyfile").filter(|k| !k.is_empty()) {
                Some(path) => SshAuth::KeyFile { path },
                None => SshAuth::Password,
            };
            Endpoint::Sftp {
                host: host.into(),
                port: port(s.child_text("Port").as_deref(), 22)?,
                user,
                auth,
            }
        }
        // FTP and FTP over TLS share protocol 0, and the export's encryption
        // field is not read here. Importing as plain FTP could downgrade a
        // secure server, so these are left for the user to add.
        "0" => {
            return Err(
                "FTP entries are not imported: FileZilla's encryption setting is not read, \
                 so add them by hand to choose FTPS or plain FTP"
                    .into(),
            )
        }
        other => return Err(format!("protocol {other} is not supported")),
    };
    Ok((connection(label, host, endpoint), secret))
}

/// FileZilla stores passwords as base64 (`encoding="base64"`), or as plain text.
fn filezilla_password(pass: &Node) -> Option<String> {
    // Only the base64 form is trimmed; a plain-text password keeps its spaces.
    let value = if pass.attr("encoding") == Some("base64") {
        String::from_utf8(base64_decode(pass.text.trim())?).ok()?
    } else {
        pass.text.clone()
    };
    (!value.is_empty()).then_some(value)
}

// ---- Cyberduck ---------------------------------------------------------------

/// Parse Cyberduck's bookmark files (a plist array of bookmark dictionaries).
/// Cyberduck keeps passwords in the system keychain, so none are imported.
pub fn parse_cyberduck_bookmarks(xml: &str) -> ImportReport {
    let root = match parse_xml(xml) {
        Ok(root) => root,
        Err(e) => return ImportReport::unreadable(format!("not readable as XML: {e}")),
    };
    let Some(array) = root.child("plist").and_then(|p| p.child("array")) else {
        return ImportReport::unreadable("not a Cyberduck bookmark file (no list of bookmarks)");
    };
    let mut report = ImportReport::default();
    for dict in array.children.iter().filter(|n| n.name == "dict") {
        let values = plist_dict(dict);
        let host = values.get("Hostname").map(|h| h.as_str()).unwrap_or("");
        let name = values
            .get("Nickname")
            .filter(|n| !n.is_empty())
            .map(|n| n.as_str())
            .unwrap_or(host);
        let draft = cyberduck_bookmark(name, host, &values);
        report.add(name, draft);
    }
    report
}

fn cyberduck_bookmark(name: &str, host: &str, v: &HashMap<String, String>) -> Draft {
    if host.is_empty() {
        return Err("the bookmark has no host name".into());
    }
    let user = v.get("Username").map(|u| u.as_str()).unwrap_or("");
    let port_text = v.get("Port").map(|p| p.as_str()).filter(|p| !p.is_empty());
    let endpoint = match v.get("Protocol").map(|p| p.as_str()).unwrap_or("") {
        "sftp" => {
            if user.is_empty() {
                return Err("the bookmark has no user name".into());
            }
            Endpoint::Sftp {
                host: host.into(),
                port: port(port_text, 22)?,
                user: user.into(),
                auth: SshAuth::Password,
            }
        }
        protocol @ ("ftp" | "ftp-tls" | "ftp-ssl") => {
            let security = match protocol {
                "ftp" => FtpSecurity::None,
                "ftp-tls" => FtpSecurity::Explicit,
                _ => FtpSecurity::Implicit,
            };
            let default_port = if matches!(security, FtpSecurity::Implicit) {
                990
            } else {
                21
            };
            Endpoint::Ftp {
                host: host.into(),
                port: port(port_text, default_port)?,
                user: user.into(),
                security,
            }
        }
        protocol @ ("dav" | "dav-ssl") => {
            let scheme = if protocol == "dav-ssl" {
                "https"
            } else {
                "http"
            };
            let default_port = if protocol == "dav-ssl" { 443 } else { 80 };
            let port = port(port_text, default_port)?;
            let host_part = if port == default_port {
                host.to_string()
            } else {
                format!("{host}:{port}")
            };
            let path = v
                .get("Path")
                .map(|p| p.as_str())
                .filter(|p| !p.is_empty())
                .unwrap_or("/");
            Endpoint::WebDav {
                url: format!("{scheme}://{host_part}{path}"),
                user: user.into(),
            }
        }
        "s3" => return Err("S3 bookmarks are not imported from Cyberduck yet".into()),
        "" => return Err("the bookmark does not name a protocol".into()),
        other => return Err(format!("protocol {other} is not supported")),
    };
    Ok((connection(name, host, endpoint), None))
}

/// The key/value pairs of a plist `<dict>`. Values are kept as their text.
fn plist_dict(dict: &Node) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut key: Option<String> = None;
    for child in &dict.children {
        if child.name == "key" {
            key = Some(child.text.trim().to_string());
        } else if let Some(k) = key.take() {
            values.insert(k, child.text.trim().to_string());
        }
    }
    values
}

// ---- shared helpers ----------------------------------------------------------

/// A trimmed, non-empty value from an INI section.
fn field<'a>(values: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    values.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = hex_pair(&bytes[i + 1..i + 3]) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(pair: &[u8]) -> Option<u8> {
    std::str::from_utf8(pair)
        .ok()
        .and_then(|h| u8::from_str_radix(h, 16).ok())
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes().chunks(2).map(hex_pair).collect()
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---- a small XML reader ------------------------------------------------------
//
// Enough XML for site manager files and plists: elements, attributes, text,
// comments, declarations, CDATA and the five predefined entities. It does not
// check DTDs or namespaces.

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    /// Text directly inside the element (not inside its children).
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn child_text(&self, name: &str) -> Option<String> {
        self.child(name).map(|c| c.text.trim().to_string())
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn parse_xml(input: &str) -> Result<Node, String> {
    let mut stack = vec![Node::default()];
    let mut rest = input;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut stack, rest);
            break;
        };
        push_text(&mut stack, &rest[..lt]);
        rest = &rest[lt..];

        if let Some(r) = rest.strip_prefix("<!--") {
            let end = r.find("-->").ok_or("unterminated comment")?;
            rest = &r[end + 3..];
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").ok_or("unterminated CDATA")?;
            stack.last_mut().unwrap().text.push_str(&r[..end]);
            rest = &r[end + 3..];
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest.find('>').ok_or("unterminated declaration")?;
            rest = &rest[end + 1..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or("unterminated closing tag")?;
            let name = r[..end].trim();
            rest = &r[end + 1..];
            if stack.len() < 2 {
                return Err(format!("closing tag </{name}> has no opening tag"));
            }
            let node = stack.pop().unwrap();
            if node.name != name {
                return Err(format!("<{}> closed by </{name}>", node.name));
            }
            stack.last_mut().unwrap().children.push(node);
        } else {
            let end = tag_end(&rest[1..]).ok_or("unterminated tag")?;
            let inner = &rest[1..1 + end];
            rest = &rest[end + 2..];
            let self_closing = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let (name, attr_text) = match inner.find(char::is_whitespace) {
                Some(i) => (&inner[..i], &inner[i..]),
                None => (inner, ""),
            };
            if name.is_empty() {
                return Err("tag without a name".into());
            }
            let node = Node {
                name: name.to_string(),
                attrs: parse_attrs(attr_text),
                ..Node::default()
            };
            if self_closing {
                stack.last_mut().unwrap().children.push(node);
            } else {
                stack.push(node);
            }
        }
    }
    if stack.len() != 1 {
        return Err(format!("<{}> is never closed", stack.last().unwrap().name));
    }
    Ok(stack.pop().unwrap())
}

fn push_text(stack: &mut [Node], raw: &str) {
    if !raw.is_empty() {
        stack.last_mut().unwrap().text.push_str(&unescape(raw));
    }
}

/// Index of the `>` that ends a start tag, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '>' => return Some(i),
            None => {}
        }
    }
    None
}

fn parse_attrs(mut s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        let Some(eq) = s.find('=') else { break };
        let name = s[..eq].trim().to_string();
        let after = s[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let body = &after[1..];
        let Some(end) = body.find(quote) else { break };
        out.push((name, unescape(&body[..end])));
        s = &body[end + 1..];
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        match after
            .find(';')
            .and_then(|end| entity(&after[..end]).map(|c| (c, end)))
        {
            Some((c, end)) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}
