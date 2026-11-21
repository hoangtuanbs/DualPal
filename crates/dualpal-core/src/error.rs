use serde::{Serialize, Serializer};

/// Errors produced by file systems and operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("unknown file system: {0}")]
    UnknownFileSystem(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error("could not connect: {0}")]
    Connection(String),
    #[error("sign-in failed: {0}")]
    AuthFailed(String),
    /// The server's host key is not in known_hosts yet; the user must confirm it.
    #[error("unknown host {host}, key fingerprint {fingerprint}")]
    UnknownHost { host: String, fingerprint: String },
    /// The server's host key differs from the one in known_hosts.
    #[error("the host key of {host} has changed (now {fingerprint}); this could be an attack")]
    HostKeyChanged { host: String, fingerprint: String },
    /// Saved passwords are protected by a master password that has not been
    /// entered yet in this run of the app.
    #[error("saved passwords are locked; enter the master password to unlock them")]
    Locked,
    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// Several items failed (an operation carried on past each failure).
    /// Holds at least two errors; its kind is the first one's.
    #[error("{} items failed: {}", .0.len(), list_errors(.0))]
    Several(Vec<Error>),
}

/// The first few errors, then how many more.
fn list_errors(errors: &[Error]) -> String {
    const SHOWN: usize = 5;
    let mut s = errors
        .iter()
        .take(SHOWN)
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    if errors.len() > SHOWN {
        s.push_str(&format!("; and {} more", errors.len() - SHOWN));
    }
    s
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Map an `std::io::Error` to the most specific variant.
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        let path = path.into();
        match source.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(path),
            std::io::ErrorKind::AlreadyExists => Error::AlreadyExists(path),
            std::io::ErrorKind::PermissionDenied => Error::PermissionDenied(path),
            _ => Error::Io { path, source },
        }
    }

    /// Short machine-readable error kind, used by front ends.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "notFound",
            Error::AlreadyExists(_) => "alreadyExists",
            Error::PermissionDenied(_) => "permissionDenied",
            Error::UnknownFileSystem(_) => "unknownFileSystem",
            Error::Invalid(_) => "invalid",
            Error::Unsupported(_) => "unsupported",
            Error::Cancelled => "cancelled",
            Error::Connection(_) => "connection",
            Error::AuthFailed(_) => "authFailed",
            Error::UnknownHost { .. } => "unknownHost",
            Error::HostKeyChanged { .. } => "hostKeyChanged",
            Error::Locked => "locked",
            Error::Io { .. } => "io",
            Error::Several(errors) => errors.first().map_or("io", Error::kind),
        }
    }

    /// One error for a list of failures: `None` when empty, the error
    /// itself when there is one, [`Error::Several`] otherwise.
    pub fn combine(mut errors: Vec<Error>) -> Option<Error> {
        match errors.len() {
            0 => None,
            1 => errors.pop(),
            _ => Some(Error::Several(errors)),
        }
    }
}

/// Serialized as `{ "kind": "...", "message": "..." }` so the UI can react to
/// specific failures (for example offer "overwrite?" on `alreadyExists`).
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Error", 3)?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        if let Error::UnknownHost { fingerprint, .. } | Error::HostKeyChanged { fingerprint, .. } =
            self
        {
            st.serialize_field("fingerprint", fingerprint)?;
        }
        st.end()
    }
}

/// A cloneable, serializable snapshot of an [`Error`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ErrorInfo {
    pub kind: String,
    pub message: String,
}

impl From<&Error> for ErrorInfo {
    fn from(e: &Error) -> Self {
        Self {
            kind: e.kind().to_string(),
            message: e.to_string(),
        }
    }
}
