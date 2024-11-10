use std::fmt::Debug;

use bytes::Bytes;
pub const HELLO_SIGNAL: &[u8] = b"crabmic";
pub const CRABMIC_ALPN: &[u8] = b"crabmic";
pub const HELLO_SIGNAL_SIZE: usize = HELLO_SIGNAL.len();
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(0, 1, 0);
/// unique identifier for a audio room user
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RoomUserId(u64);

impl Debug for RoomUserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RoomUserId({:08x})", self.0)
    }
}

impl std::fmt::Display for RoomUserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:08x}", self.0)
    }
}

impl From<u64> for RoomUserId {
    fn from(value: u64) -> Self {
        RoomUserId(value)
    }
}

impl From<RoomUserId> for u64 {
    fn from(val: RoomUserId) -> Self {
        val.0
    }
}

impl RoomUserId {
    pub fn next(&self) -> RoomUserId {
        RoomUserId(self.0 + 1)
    }
    pub const fn into_inner(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct RoomAudioPacket {
    pub user_id: RoomUserId,
    pub data: Bytes,
}

impl RoomAudioPacket {
    pub fn new(user_id: impl Into<RoomUserId>, data: impl Into<Bytes>) -> Self {
        Self {
            user_id: user_id.into(),
            data: data.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProtocolVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
    pub fn major(&self) -> u16 {
        self.major
    }
    pub fn minor(&self) -> u16 {
        self.minor
    }
    pub fn patch(&self) -> u16 {
        self.patch
    }
    pub const fn to_bytes(&self) -> [u8; 6] {
        let mut bytes = [0; 6];
        bytes[0] = (self.major >> 8) as u8;
        bytes[1] = self.major as u8;
        bytes[2] = (self.minor >> 8) as u8;
        bytes[3] = self.minor as u8;
        bytes[4] = (self.patch >> 8) as u8;
        bytes[5] = self.patch as u8;
        bytes
    }
    pub fn from_bytes(bytes: [u8; 6]) -> Self {
        Self {
            major: (u16::from(bytes[0]) << 8) | u16::from(bytes[1]),
            minor: (u16::from(bytes[2]) << 8) | u16::from(bytes[3]),
            patch: (u16::from(bytes[4]) << 8) | u16::from(bytes[5]),
        }
    }
    pub fn compatible(&self, other: &Self) -> bool {
        self.major == other.major && self.minor == other.minor
    }
}


impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
#[macro_export]
macro_rules! cancellable {
    ($ct:expr, $next:expr) => {
        tokio::select! {
            _ = $ct.cancelled() => {
                break;
            }
            next = $next => {
                next
            }
        }
    };
}
