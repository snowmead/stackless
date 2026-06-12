use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeError {
    InvalidPid { value: u32 },
    InvalidProcessStartTime { value: u64 },
    InvalidTcpPort { value: u16 },
    InvalidHttpStatus { value: u16 },
    InvalidProtocolVersion { value: u32 },
    InvalidDnsName { value: String, detail: String },
    InvalidProxyHost { value: String, detail: String },
    InvalidLogPath { value: String, detail: String },
    InvalidContainerId { value: String },
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPid { value } => write!(f, "invalid pid {value}"),
            Self::InvalidProcessStartTime { value } => {
                write!(f, "invalid process start time {value}")
            }
            Self::InvalidTcpPort { value } => write!(f, "invalid TCP port {value}"),
            Self::InvalidHttpStatus { value } => write!(f, "invalid HTTP status {value}"),
            Self::InvalidProtocolVersion { value } => {
                write!(f, "unsupported protocol version {value}")
            }
            Self::InvalidDnsName { value, detail } => {
                write!(f, "invalid DNS name {value:?}: {detail}")
            }
            Self::InvalidProxyHost { value, detail } => {
                write!(f, "invalid proxy host {value:?}: {detail}")
            }
            Self::InvalidLogPath { value, detail } => {
                write!(f, "invalid log path {value:?}: {detail}")
            }
            Self::InvalidContainerId { value } => write!(f, "invalid container id {value:?}"),
        }
    }
}

impl std::error::Error for TypeError {}
