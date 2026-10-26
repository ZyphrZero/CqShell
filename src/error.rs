use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Error {
    pub key: &'static str,
    pub detail: String,
}

impl Error {
    pub fn new(key: &'static str, detail: impl ToString) -> Self {
        Self {
            key,
            detail: detail.to_string(),
        }
    }

    pub fn with_recovery(self, recovery: Error, path: &std::path::Path) -> Self {
        Self::new(
            "error.recovery",
            format!("{}; {}; {}", self, recovery, path.display()),
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.detail)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new("error.io", value)
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::new("error.data", value)
    }
}
impl From<toml::de::Error> for Error {
    fn from(value: toml::de::Error) -> Self {
        Self::new("error.data", value)
    }
}
impl From<toml::ser::Error> for Error {
    fn from(value: toml::ser::Error) -> Self {
        Self::new("error.data", value)
    }
}
