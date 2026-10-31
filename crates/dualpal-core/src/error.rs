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
    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
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
            Error::Io { .. } => "io",
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
