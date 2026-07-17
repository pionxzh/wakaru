use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidOptions,
    InvalidInput,
    Parse,
    SourceMap,
    Emit,
    Internal,
}

pub(crate) fn from_core_driver_error(kind: wakaru_core::driver::DriverErrorKind) -> ErrorKind {
    match kind {
        wakaru_core::driver::DriverErrorKind::Parse => ErrorKind::Parse,
        wakaru_core::driver::DriverErrorKind::SourceMap => ErrorKind::SourceMap,
        wakaru_core::driver::DriverErrorKind::InvalidInput => ErrorKind::InvalidInput,
        wakaru_core::driver::DriverErrorKind::InvalidOptions => ErrorKind::InvalidOptions,
        wakaru_core::driver::DriverErrorKind::Internal => ErrorKind::Internal,
    }
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    input_filename: Option<String>,
    source: anyhow::Error,
}

impl Error {
    pub(crate) fn new(
        kind: ErrorKind,
        input_filename: Option<String>,
        source: impl Into<anyhow::Error>,
    ) -> Self {
        Self {
            kind,
            input_filename,
            source: source.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn input_filename(&self) -> Option<&str> {
        self.input_filename.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.source()
    }
}
