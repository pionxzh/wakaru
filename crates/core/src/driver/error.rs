use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverErrorKind {
    Parse,
    SourceMap,
    InvalidInput,
    InvalidOptions,
    Internal,
}

#[derive(Debug)]
pub struct DriverError {
    kind: DriverErrorKind,
    error: anyhow::Error,
}

impl DriverError {
    pub(crate) fn new(kind: DriverErrorKind, error: impl Into<anyhow::Error>) -> Self {
        Self {
            kind,
            error: error.into(),
        }
    }

    pub fn kind(&self) -> DriverErrorKind {
        self.kind
    }

    pub fn into_inner(self) -> anyhow::Error {
        self.error
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for DriverError {}

pub type DriverResult<T> = std::result::Result<T, DriverError>;
