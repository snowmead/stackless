//! Checkpoint payload schemas shared across substrates and the daemon.

use serde::{Deserialize, Serialize};

use crate::types::{LogPath, Pid, ProcessStartTime, ProxyHost, TcpPort};

/// What a `start:` checkpoint records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartCheckpoint {
    pub pid: Pid,
    pub start_time: ProcessStartTime,
    pub port: TcpPort,
    pub hosts: Vec<ProxyHost>,
    pub log: LogPath,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_checkpoint_roundtrip() {
        let original = StartCheckpoint {
            pid: Pid::from_os(12345),
            start_time: ProcessStartTime::from_os(1_700_000_000),
            port: TcpPort::try_new(8080).unwrap(),
            hosts: vec![
                ProxyHost::try_new("api.dev.localhost").unwrap(),
                ProxyHost::try_new("dev.localhost").unwrap(),
            ],
            log: LogPath::try_new("/tmp/state/logs/dev/api.log").unwrap(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: StartCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
        assert!(json.contains("\"pid\":12345"));
        assert!(json.contains("\"port\":8080"));
    }
}