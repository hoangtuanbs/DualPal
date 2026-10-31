//! Path helpers for backends that use `/`-separated paths (SFTP, S3).
//! Paths are absolute, start with `/` and never end with one, except the root.

/// Normalize to an absolute path without a trailing slash, resolving `.`
/// and `..` segments.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent(path: &str) -> Option<String> {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        return None;
    }
    match p.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(p[..i].to_string()),
        None => None,
    }
}

pub fn file_name(path: &str) -> Option<String> {
    let p = path.trim_end_matches('/');
    let name = p.rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

pub fn is_within(path: &str, ancestor: &str) -> bool {
    let a = ancestor.trim_end_matches('/');
    if a.is_empty() {
        return path.starts_with('/');
    }
    path == a
        || path
            .strip_prefix(a)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(normalize("a/./b/../c/"), "/a/c");
        assert_eq!(normalize("/"), "/");
        assert_eq!(join("/", "x"), "/x");
        assert_eq!(join("/a", "x"), "/a/x");
        assert_eq!(parent("/a/b"), Some("/a".into()));
        assert_eq!(parent("/a"), Some("/".into()));
        assert_eq!(parent("/"), None);
        assert_eq!(file_name("/a/b"), Some("b".into()));
        assert_eq!(file_name("/"), None);
        assert!(is_within("/a/b", "/a"));
        assert!(is_within("/a", "/a"));
        assert!(!is_within("/ab", "/a"));
        assert!(is_within("/ab", "/"));
    }
}
