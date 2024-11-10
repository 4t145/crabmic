use std::borrow::Cow;

use quinn::{crypto::rustls::{self, NoInitialCipherSuite}, ConnectionError, ReadError, ReadExactError, WriteError};
#[derive(Debug)]
pub struct InternalError {
    kind: InternalErrorKind,
    context: Cow<'static, str>,
}
impl std::fmt::Display for InternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.context)
    }
}
impl InternalError {
    pub fn new(kind: InternalErrorKind, context: impl Into<Cow<'static, str>>) -> Self {
        InternalError {
            kind,
            context: context.into(),
        }
    }
    pub fn protocol(context: impl Into<Cow<'static, str>>) -> Self {
        InternalError {
            kind: InternalErrorKind::Protocol,
            context: context.into(),
        }
    }
    pub fn contextual<E, C>(context: C) -> impl FnOnce(E) -> Self
    where
        E: Into<InternalErrorKind>,
        C: Into<Cow<'static, str>>,
    {
        move |e| InternalError {
            kind: e.into(),
            context: context.into(),
        }
    }
}
#[derive(Debug)]
pub enum InternalErrorKind {
    Quic(QuicError),
    Io(std::io::Error),
    Protocol,
}

impl std::fmt::Display for InternalErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InternalErrorKind::Quic(e) => write!(f, "Quic error: {:?}", e),
            InternalErrorKind::Io(e) => write!(f, "Io error: {}", e),
            InternalErrorKind::Protocol => write!(f, "Protocol error"),
        }
    }
}


impl From<std::io::Error> for InternalErrorKind {
    fn from(e: std::io::Error) -> Self {
        InternalErrorKind::Io(e)
    }
}
impl From<WriteError> for InternalErrorKind {
    fn from(e: WriteError) -> Self {
        InternalErrorKind::Quic(QuicError::WriteError(e))
    }
}

impl From<ConnectionError> for InternalErrorKind {
    fn from(e: ConnectionError) -> Self {
        InternalErrorKind::Quic(QuicError::ConnectionError(e))
    }
}

impl From<QuicError> for InternalErrorKind {
    fn from(e: QuicError) -> Self {
        InternalErrorKind::Quic(e)
    }
}

impl From<rustls::Error> for InternalErrorKind {
    fn from(e: rustls::Error) -> Self {
        InternalErrorKind::Quic(QuicError::Rustls(e))
    }
}

impl From<ReadError> for InternalErrorKind {
    fn from(e: ReadError) -> Self {
        InternalErrorKind::Quic(QuicError::ReadError(e))
    }
}
impl From<ReadExactError> for InternalErrorKind {
    fn from(e: ReadExactError) -> Self {
        InternalErrorKind::Quic(QuicError::ReadExactError(e))
    }
}
impl From<NoInitialCipherSuite> for InternalErrorKind {
    fn from(e: NoInitialCipherSuite) -> Self {
        InternalErrorKind::Quic(QuicError::NoInitialCipherSuite(e))
    }
}

impl From<WriteError> for QuicError {
    fn from(e: WriteError) -> Self {
        QuicError::WriteError(e)
    }
}
#[derive(Debug)]
pub enum QuicError {
    WriteError(WriteError),
    ReadError(ReadError),
    ReadExactError(ReadExactError),
    ConnectionError(ConnectionError),
    ZeroRttRejected,
    ConnectionLost,
    ClosedStream,
    Rustls(rustls::Error),
    NoInitialCipherSuite(NoInitialCipherSuite),
}
