use std::{
    error::Error,
    io::{Read, Write},
};

use serde::{Deserialize, Serialize};

pub const DEFAULT_UNIX_SOCKET_PATH: &str = "/run/backlightd.sock";

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum BacklightMode {
    Auto,
    Manual,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BacklightInfo {
    pub brightness_percent: u8,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum BacklightCommand {
    SetBrightness(u8),
    IncreaseBrightness(u8),
    DecreaseBrightness(u8),
    TurnOffMonitors,
    TurnOnMonitors,
    Refresh,
    SetMode(BacklightMode),
    GetInfo,
    GetInfoResponse(BacklightInfo),
    NotifyShutdown,
}

// The following abstraction allow us to easily change the protocol if need be.
// Crates that use the BacklightCommand enum don't need to know that bincode is used under the hood.
impl BacklightCommand {
    pub fn serialize_into(&self, writer: impl Write) -> Result<(), Box<dyn Error>> {
        Ok(bincode::serialize_into(writer, self)?)
    }
    pub fn deserialize_from(reader: impl Read) -> Result<Self, Box<dyn Error>> {
        Ok(bincode::deserialize_from(reader)?)
    }
}
