use std::borrow::Cow;

use quinn::{ConnectionError, WriteError};

pub struct InternalError {
    kind: InternalErrorKind,
    context: Cow<'static, str>,
}

impl InternalError {
    pub fn new(kind: InternalErrorKind, context: impl Into<Cow<'static, str>>) -> Self {
        InternalError {
            kind,
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

pub enum InternalErrorKind {
    Quic(QuicError),
    Io(std::io::Error),
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

impl From<WriteError> for QuicError {
    fn from(e: WriteError) -> Self {
        QuicError::WriteError(e)
    }
}
#[derive(Debug)]
pub enum QuicError {
    WriteError(WriteError),
    ConnectionError(ConnectionError),
    ZeroRttRejected,
    ConnectionLost,
    ClosedStream,
}
