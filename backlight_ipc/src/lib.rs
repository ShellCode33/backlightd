use std::{
    error::Error,
    fmt::Display,
    io::{self, BufRead, Write},
};

use serde::{Deserialize, Serialize};

/// Maximum size of a JSON message, including its terminating newline.
pub const MAX_MESSAGE_BYTES: usize = 4096;

pub const DEFAULT_UNIX_SOCKET_PATH: &str = "/run/backlightd.sock";

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BacklightMode {
    Auto,
    Manual,
}

impl Display for BacklightMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BacklightMode::Auto => write!(f, "auto"),
            BacklightMode::Manual => write!(f, "manual"),
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub struct BacklightInfo {
    pub brightness_percent: u8,
    pub mode: BacklightMode,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum BacklightCommand {
    SetBrightness(u8),
    IncreaseBrightness(u8),
    DecreaseBrightness(u8),
    TurnOffMonitors,
    TurnOnMonitors,
    Refresh,
    NotifyHotplug,
    SetMode(BacklightMode),
    GetInfo,
    GetInfoResponse(BacklightInfo),
    NotifyShutdown,
    ErrorResponse(String),
}

// One compact JSON object per line. Keep a single buffered reader for each
// connection so that reading one message never discards bytes from the next.
impl BacklightCommand {
    pub fn serialize_into(&self, mut writer: impl Write) -> Result<(), Box<dyn Error>> {
        let mut message = serde_json::to_vec(self)?;
        if message.len() >= MAX_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC message exceeds size limit",
            )
            .into());
        }
        message.push(b'\n');
        writer.write_all(&message)?;
        writer.flush()?;
        Ok(())
    }

    pub fn deserialize_from(reader: impl BufRead) -> Result<Self, Box<dyn Error>> {
        let mut message = Vec::new();
        reader
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_until(b'\n', &mut message)?;
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC message exceeds size limit",
            )
            .into());
        }
        if message.last() != Some(&b'\n') {
            return Err(
                io::Error::new(io::ErrorKind::UnexpectedEof, "Incomplete IPC message").into(),
            );
        }
        Ok(serde_json::from_slice(&message)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn json_wire_format_and_consecutive_messages() {
        let commands = [
            (
                BacklightCommand::SetBrightness(75),
                r#"{"type":"set_brightness","value":75}"#,
            ),
            (
                BacklightCommand::IncreaseBrightness(10),
                r#"{"type":"increase_brightness","value":10}"#,
            ),
            (
                BacklightCommand::DecreaseBrightness(10),
                r#"{"type":"decrease_brightness","value":10}"#,
            ),
            (
                BacklightCommand::TurnOffMonitors,
                r#"{"type":"turn_off_monitors"}"#,
            ),
            (
                BacklightCommand::TurnOnMonitors,
                r#"{"type":"turn_on_monitors"}"#,
            ),
            (BacklightCommand::Refresh, r#"{"type":"refresh"}"#),
            (
                BacklightCommand::NotifyHotplug,
                r#"{"type":"notify_hotplug"}"#,
            ),
            (
                BacklightCommand::SetMode(BacklightMode::Auto),
                r#"{"type":"set_mode","value":"auto"}"#,
            ),
            (
                BacklightCommand::SetMode(BacklightMode::Manual),
                r#"{"type":"set_mode","value":"manual"}"#,
            ),
            (BacklightCommand::GetInfo, r#"{"type":"get_info"}"#),
            (
                BacklightCommand::GetInfoResponse(BacklightInfo {
                    brightness_percent: 50,
                    mode: BacklightMode::Manual,
                }),
                r#"{"type":"get_info_response","value":{"brightness_percent":50,"mode":"manual"}}"#,
            ),
            (
                BacklightCommand::NotifyShutdown,
                r#"{"type":"notify_shutdown"}"#,
            ),
            (
                BacklightCommand::ErrorResponse("oops\nagain".into()),
                r#"{"type":"error_response","value":"oops\nagain"}"#,
            ),
        ];
        let mut stream = Vec::new();
        for (command, expected) in &commands {
            let mut encoded = Vec::new();
            command.serialize_into(&mut encoded).unwrap();
            assert_eq!(encoded, format!("{expected}\n").as_bytes());
            stream.extend(encoded);
        }
        // A tiny buffer also exercises messages split across several reads.
        let mut reader = BufReader::with_capacity(7, Cursor::new(stream));
        for (_, expected) in commands {
            let command = BacklightCommand::deserialize_from(&mut reader).unwrap();
            assert_eq!(serde_json::to_string(&command).unwrap(), expected);
        }
        assert!(BacklightCommand::deserialize_from(&mut reader).is_err());
    }

    #[test]
    fn rejects_invalid_and_incomplete_messages() {
        for input in [
            "",
            "{\n",
            "{}\n",
            "{\"type\":\"get_info\"}",
            "{\"type\":\"unknown\"}\n",
            "{\"type\":\"set_brightness\",\"value\":256}\n",
        ] {
            assert!(
                BacklightCommand::deserialize_from(Cursor::new(input)).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn message_size_is_bounded_on_both_sides() {
        let prefix = r#"{"type":"error_response","value":""#;
        let suffix = "\"}\n";
        let payload = MAX_MESSAGE_BYTES - prefix.len() - suffix.len();
        let valid = format!("{prefix}{}{suffix}", "x".repeat(payload));
        assert_eq!(valid.len(), MAX_MESSAGE_BYTES);
        let command = BacklightCommand::deserialize_from(Cursor::new(&valid)).unwrap();
        let mut encoded = Vec::new();
        command.serialize_into(&mut encoded).unwrap();
        assert_eq!(encoded, valid.as_bytes());

        let oversized = format!("{prefix}{}{suffix}", "x".repeat(payload + 1));
        assert!(BacklightCommand::deserialize_from(Cursor::new(oversized)).is_err());
        assert!(BacklightCommand::ErrorResponse("x".repeat(payload + 1))
            .serialize_into(Vec::new())
            .is_err());
        assert!(
            BacklightCommand::deserialize_from(Cursor::new(vec![b'x'; MAX_MESSAGE_BYTES + 1]))
                .is_err()
        );
    }
}
