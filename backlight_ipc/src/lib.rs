//! Typed, bounded JSON messages shared by backlightd and its clients.
//!
//! # Framing and connection lifetime
//!
//! Each frame is one compact JSON object followed by a newline, bounded by
//! [`MAX_MESSAGE_BYTES`]. Preserve one buffered reader across a connection: a new
//! reader per message can discard bytes already buffered from subsequent frames.
//! A successful mutation has no response frame; send [`BacklightCommand::GetInfo`]
//! afterward to obtain status. An error returns [`BacklightCommand::ErrorResponse`]
//! and ends the connection. Clients should set appropriate socket timeouts.
//!
//! ```
//! use std::io::{BufReader, Cursor};
//! use backlight_ipc::BacklightCommand;
//!
//! let mut wire = Vec::new();
//! BacklightCommand::SetBrightness(60).serialize_into(&mut wire)?;
//! BacklightCommand::GetInfo.serialize_into(&mut wire)?;
//! let mut reader = BufReader::new(Cursor::new(wire));
//! assert!(matches!(BacklightCommand::deserialize_from(&mut reader)?,
//!     BacklightCommand::SetBrightness(60)));
//! assert!(matches!(BacklightCommand::deserialize_from(&mut reader)?,
//!     BacklightCommand::GetInfo));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Unix-socket client
//!
//! This read-only example is compiled but not executed by documentation tests.
//!
//! ```no_run
//! use std::{io::BufReader, os::unix::net::UnixStream, time::Duration};
//! use backlight_ipc::{BacklightCommand, DEFAULT_UNIX_SOCKET_PATH};
//!
//! let stream = UnixStream::connect(DEFAULT_UNIX_SOCKET_PATH)?;
//! stream.set_read_timeout(Some(Duration::from_secs(5)))?;
//! stream.set_write_timeout(Some(Duration::from_secs(5)))?;
//! let mut reader = BufReader::new(&stream);
//! BacklightCommand::GetInfo.serialize_into(&stream)?;
//! match BacklightCommand::deserialize_from(&mut reader)? {
//!     BacklightCommand::GetInfoResponse(info) => {
//!         println!("{}% ({})", info.brightness_percent, info.mode);
//!         BacklightCommand::NotifyShutdown.serialize_into(&stream)?;
//!     }
//!     BacklightCommand::ErrorResponse(message) => return Err(message.into()),
//!     unexpected => return Err(format!("Unexpected response: {unexpected:?}").into()),
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Protocol boundaries
//!
//! Commands serialize with `type` and optional `value` fields in snake case.
//! Serialization validates framing size, not command semantics: for example, a
//! typed `SetBrightness(101)` can be encoded but the daemon rejects it. This is a
//! pre-1.0 protocol with no version-negotiation handshake or bincode compatibility.
//! `NotifyShutdown` closes a connection, not the daemon. Socket permissions are
//! the access-control boundary; the protocol itself provides no authentication.
#![warn(missing_docs)]

use std::{
    error::Error,
    fmt::Display,
    io::{self, BufRead, Write},
};

use serde::{Deserialize, Serialize};

/// Maximum size of a JSON message, including its terminating newline.
pub const MAX_MESSAGE_BYTES: usize = 4096;

/// Default filesystem address of the system daemon.
pub const DEFAULT_UNIX_SOCKET_PATH: &str = "/run/backlightd.sock";

/// Whether brightness follows the schedule or a manual override.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BacklightMode {
    /// Follow the configured automatic schedule.
    Auto,
    /// Keep manual intent until its timeout, or indefinitely when configured.
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

#[derive(Clone, Serialize, Deserialize, Debug)]
/// Cached details for one discovered device; no field promises physical luminance.
///
/// Requested, representable, control-readback, and actual driver readings have
/// different meanings. Brightness writes invalidate control-readback evidence
/// until rediscovery.
pub struct DeviceInfo {
    /// Stable configuration selector; EDID fingerprints can be shared by identical EDIDs.
    pub id: String,
    /// Human-readable model or sysfs device name; it need not be unique.
    pub name: String,
    /// Whether writes and inclusion in the group average are enabled.
    pub enabled: bool,
    /// Last successfully accepted logical target, when retained.
    pub requested_percent: Option<u8>,
    /// Representable logical percentage after calibration and hardware quantization.
    pub brightness_percent: u8,
    /// Stored raw control value as a rounded percentage of the hardware maximum.
    pub hardware_percent: u8,
    /// Last read or successfully written raw control value; a write is not verification.
    pub hardware_raw: u32,
    /// Device control maximum; sysfs is not limited to 16 bits.
    pub maximum_raw: u32,
    /// Control value read during discovery; None after an unverified brightness write.
    pub readback_raw: Option<u32>,
    /// Optional sysfs actual_brightness reading; not a luminance measurement.
    pub actual_raw: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
/// Group status, optionally carrying one indexed device record.
///
/// Normal status omits `device` from JSON. Device enumeration ends when the
/// requested index has no record and this field is None.
pub struct BacklightInfo {
    /// Integer average of representable logical percentages across enabled devices.
    pub brightness_percent: u8,
    /// Current controller mode.
    pub mode: BacklightMode,
    /// Indexed device details for GetDeviceInfo; absent for ordinary status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
/// Commands and responses in a newline-delimited, adjacently tagged JSON stream.
pub enum BacklightCommand {
    /// Set a logical target from 0 through 100 and enter manual mode.
    SetBrightness(u8),
    /// Increase each device target by percentage points, saturating at 100.
    IncreaseBrightness(u8),
    /// Decrease each device target by percentage points, stopping at 1.
    DecreaseBrightness(u8),
    /// Request power-off for enabled devices without changing brightness targets.
    TurnOffMonitors,
    /// Request power-on for enabled devices.
    TurnOnMonitors,
    /// Rediscover devices synchronously; errors remain visible to the caller.
    Refresh,
    /// Queue a debounced hotplug notification without a completion/status reply.
    NotifyHotplug,
    /// Select automatic control or capture a manual target.
    SetMode(BacklightMode),
    /// Request a GetInfoResponse containing group status.
    GetInfo,
    /// Request one device by zero-based index in GetInfoResponse.device.
    GetDeviceInfo(usize),
    /// Daemon status response; optional device data supports bounded enumeration.
    GetInfoResponse(BacklightInfo),
    /// End this client connection; does not terminate the daemon.
    NotifyShutdown,
    /// Daemon error response, followed by connection closure.
    ErrorResponse(String),
}

// One compact JSON object per line. Keep a single buffered reader for each
// connection so that reading one message never discards bytes from the next.
impl BacklightCommand {
    /// Encode one compact JSON frame, append a newline, and flush the writer.
    ///
    /// # Errors
    /// Returns serialization, framing-size, write, or flush errors. Values are
    /// not checked against daemon policy here.
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

    /// Read one complete bounded frame from a persistent buffered reader.
    ///
    /// Pass `&mut reader` when reading several messages from one connection.
    ///
    /// # Errors
    /// Returns errors for oversized frames, missing newlines (including EOF),
    /// malformed JSON, unknown variants, invalid typed values, or reader failures.
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
                BacklightCommand::GetDeviceInfo(3),
                r#"{"type":"get_device_info","value":3}"#,
            ),
            (
                BacklightCommand::GetInfoResponse(BacklightInfo {
                    brightness_percent: 50,
                    mode: BacklightMode::Manual,
                    device: None,
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
    fn device_readings_larger_than_16_bits_round_trip() {
        let command = BacklightCommand::GetInfoResponse(BacklightInfo {
            brightness_percent: 50,
            mode: BacklightMode::Manual,
            device: Some(DeviceInfo {
                id: "acpi:test".into(),
                name: "sysfs".into(),
                enabled: true,
                requested_percent: Some(50),
                brightness_percent: 50,
                hardware_percent: 50,
                hardware_raw: 100000,
                maximum_raw: 200000,
                readback_raw: Some(100000),
                actual_raw: Some(90000),
            }),
        });
        let mut wire = Vec::new();
        command.serialize_into(&mut wire).unwrap();
        let BacklightCommand::GetInfoResponse(info) =
            BacklightCommand::deserialize_from(Cursor::new(wire)).unwrap()
        else {
            panic!("wrong response")
        };
        let device = info.device.unwrap();
        assert_eq!(device.hardware_raw, 100000);
        assert_eq!(device.maximum_raw, 200000);
        assert_eq!(device.actual_raw, Some(90000));
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
